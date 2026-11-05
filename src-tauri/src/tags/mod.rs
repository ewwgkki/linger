//! Audio tag & technical info reader (port of the web version's lib/tags.php, plus OGG/Opus).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::text::{clean_text, decode_legacy, is_broken};

mod ape;
mod asf;
mod cover;
mod flac;
mod id3;
mod mp4;
mod mpeg;
mod ogg;
mod wav;

#[cfg(test)]
pub(crate) mod testutil;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub albumartist: String,
    pub track: u32,
    /// seconds
    pub duration: f64,
    /// kbps
    pub bitrate: u32,
    /// "mp3"|"aac"|"alac"|"flac"|"pcm"|"wma"|"ape"|"ogg"|"opus"|"notaudio"|""
    pub codec: String,
    /// embedded cover with data > 100 bytes
    pub pic: bool,
}

/// Reads tags and technical info. Never panics; unreadable files give the default value.
pub fn read_audio_info(path: &Path) -> AudioInfo {
    std::panic::catch_unwind(|| read_audio_info_inner(path)).unwrap_or_default()
}

/// Embedded cover (data, mime); the front cover is preferred.
pub fn read_cover(path: &Path) -> Option<(Vec<u8>, &'static str)> {
    std::panic::catch_unwind(|| cover::read_cover(path)).ok().flatten()
}

fn read_audio_info_inner(path: &Path) -> AudioInfo {
    let mut info = AudioInfo::default();
    let Some(mut src) = Src::open(path) else { return info };
    let size = src.size;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();

    let head = src.read_at(0, 10);
    let mut id3len = 0u64;
    if head.len() == 10 && head.starts_with(b"ID3") {
        id3len = id3::tag_len(&head);
        let tag = src.read_at(0, id3len.min(16 << 20) as usize);
        info.merge(id3::parse_id3v2(&tag));
    }
    let magic = src.read_at(id3len, 4);
    if magic.starts_with(b"MZ") || magic == b"\x7FELF" || magic == b"PK\x03\x04" || magic == b"Rar!" {
        info.codec = "notaudio".into();
        return info;
    }
    let res = match &magic[..] {
        b"fLaC" => flac::read_flac(&mut src, id3len + 4, &mut info),
        b"MAC " => {
            ape::read_ape(&mut src, id3len, &mut info);
            Ok(())
        }
        b"RIFF" => wav::read_wav(&mut src, &mut info),
        b"\x30\x26\xB2\x75" => {
            asf::read_asf(&mut src, &mut info);
            Ok(())
        }
        b"OggS" => {
            ogg::read_ogg(&mut src, id3len, &mut info);
            Ok(())
        }
        _ => {
            if matches!(ext.as_str(), "m4a" | "mp4" | "aac") || src.read_at(4, 4) == b"ftyp" {
                mp4::read_mp4(&mut src, &mut info);
            } else if ext == "mp3" || ext == "mp2" {
                mpeg::read_mpeg(&mut src, id3len, &mut info);
            }
            Ok(())
        }
    };
    // A PHP exception skipped the tail tags but kept everything read so far.
    if res.is_ok() {
        read_tail_tags(&mut src, &mut info);
    }
    // PHP re-decodes fields that are not valid UTF-8; Rust strings always are, so nothing to do.
    if info.duration > 0.0 && info.bitrate == 0 {
        info.bitrate = php_round(size as f64 * 8.0 / info.duration / 1000.0);
    }
    info
}

/// ID3v1 and APEv2 at the end of the file (any format may carry them).
fn read_tail_tags(src: &mut Src, info: &mut AudioInfo) {
    let size = src.size;
    if size < 160 {
        return;
    }
    let v1 = src.read_at(size - 128, 128);
    let has_v1 = v1.starts_with(b"TAG");
    let foot_pos = size - 32 - if has_v1 { 128 } else { 0 };
    let foot = src.read_at(foot_pos, 32);
    if foot.starts_with(b"APETAGEX") {
        info.merge(ape::parse_apev2(src, foot_pos, &foot));
    }
    if has_v1 {
        info.merge(id3::parse_id3v1(&v1));
    }
}

/// Marks a point where the PHP original throws (e.g. `fread($fh, 0)`), aborting the read.
#[derive(Debug)]
pub(crate) struct Throw;

/// Raw tag values found in one tag block; `None` = not present.
#[derive(Debug, Default)]
pub(crate) struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub albumartist: Option<String>,
    pub track: Option<u32>,
    pub pic: bool,
}

impl Tags {
    /// Sets a field by its PHP key name (track values go through PHP `(int)`).
    pub fn set(&mut self, key: &str, val: String) {
        match key {
            "title" => self.title = Some(val),
            "artist" => self.artist = Some(val),
            "album" => self.album = Some(val),
            "albumartist" => self.albumartist = Some(val),
            "track" => self.track = Some(php_intval(&val)),
            _ => {}
        }
    }
    pub fn has(&self, key: &str) -> bool {
        match key {
            "title" => self.title.is_some(),
            "artist" => self.artist.is_some(),
            "album" => self.album.is_some(),
            "albumartist" => self.albumartist.is_some(),
            "track" => self.track.is_some(),
            _ => false,
        }
    }
}

impl AudioInfo {
    /// Port of `merge_tags`: only fill empty fields; a broken existing value may be replaced.
    pub(crate) fn merge(&mut self, t: Tags) {
        fn m(dst: &mut String, v: Option<String>) {
            let Some(v) = v else { return };
            if v.is_empty() || is_broken(&v) {
                return;
            }
            // PHP empty(): "" and "0" are empty
            if dst.is_empty() || dst == "0" || is_broken(dst) {
                *dst = v;
            }
        }
        m(&mut self.title, t.title);
        m(&mut self.artist, t.artist);
        m(&mut self.album, t.album);
        m(&mut self.albumartist, t.albumartist);
        if let Some(n) = t.track {
            if n != 0 && self.track == 0 {
                self.track = n;
            }
        }
        if t.pic {
            self.pic = true;
        }
    }
}

/// `clean_text(decode_legacy(b))`, the usual treatment of a raw tag value.
pub(crate) fn legacy_text(b: &[u8]) -> String {
    clean_text(&decode_legacy(b))
}

/// Buffered random-access reader; every read is clipped to the file and never fails loudly.
pub(crate) struct Src {
    r: BufReader<File>,
    pub size: u64,
}

impl Src {
    pub fn open(path: &Path) -> Option<Src> {
        let f = File::open(path).ok()?;
        let size = f.metadata().ok()?.len();
        Some(Src { r: BufReader::with_capacity(64 << 10, f), size })
    }

    /// Up to `len` bytes at `pos` (fewer at EOF or on error).
    pub fn read_at(&mut self, pos: u64, len: usize) -> Vec<u8> {
        if pos >= self.size || len == 0 {
            return Vec::new();
        }
        let len = (len as u64).min(self.size - pos);
        if self.r.seek(SeekFrom::Start(pos)).is_err() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(len as usize);
        let _ = (&mut self.r).take(len).read_to_end(&mut out);
        out
    }
}

// ---- byte helpers with PHP semantics: out-of-range reads yield 0 / empty ----

pub(crate) fn byte(b: &[u8], o: usize) -> u8 {
    b.get(o).copied().unwrap_or(0)
}

/// PHP `substr($b, $o, $len)` for non-negative offsets.
pub(crate) fn sub(b: &[u8], o: usize, len: usize) -> &[u8] {
    if o >= b.len() {
        return &[];
    }
    &b[o..o + len.min(b.len() - o)]
}

/// PHP `substr($b, $o)`.
pub(crate) fn tail(b: &[u8], o: usize) -> &[u8] {
    b.get(o..).unwrap_or(&[])
}

fn fixed<const N: usize>(b: &[u8], o: usize) -> Option<[u8; N]> {
    b.get(o..o.checked_add(N)?)?.try_into().ok()
}

pub(crate) fn u32be(b: &[u8], o: usize) -> u32 {
    fixed::<4>(b, o).map(u32::from_be_bytes).unwrap_or(0)
}
pub(crate) fn u32le(b: &[u8], o: usize) -> u32 {
    fixed::<4>(b, o).map(u32::from_le_bytes).unwrap_or(0)
}
pub(crate) fn u16le(b: &[u8], o: usize) -> u16 {
    fixed::<2>(b, o).map(u16::from_le_bytes).unwrap_or(0)
}
pub(crate) fn u64le(b: &[u8], o: usize) -> u64 {
    fixed::<8>(b, o).map(u64::from_le_bytes).unwrap_or(0)
}

/// Port of `synchsafe` (bytes are not masked, like PHP).
pub(crate) fn synchsafe(b: &[u8], o: usize) -> u64 {
    (byte(b, o) as u64) << 21 | (byte(b, o + 1) as u64) << 14 | (byte(b, o + 2) as u64) << 7 | byte(b, o + 3) as u64
}

/// PHP `(int)round($x)` clamped to u32.
pub(crate) fn php_round(x: f64) -> u32 {
    if x.is_finite() && x > 0.0 {
        x.round().min(u32::MAX as f64) as u32
    } else {
        0
    }
}

/// PHP `(int)$s` for tag strings: leading whitespace, optional sign, digits ("3/12" -> 3).
/// Negative values become 0.
pub(crate) fn php_intval(s: &str) -> u32 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0B', '\x0C']);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: &str = &t[..t.bytes().take_while(u8::is_ascii_digit).count()];
    if neg || digits.is_empty() {
        return 0;
    }
    digits.parse::<u64>().map(|v| v.min(u32::MAX as u64) as u32).unwrap_or(u32::MAX)
}

/// UTF-16 to UTF-8 the way PHP `conv()` does it: iconv with //IGNORE drops invalid units;
/// odd-length input makes iconv fail and mb_convert_encoding substitutes '?'.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Utf16 {
    /// "UTF-16": BOM decides; without BOM glibc iconv assumes little-endian
    Bom,
    Be,
    Le,
}

pub(crate) fn conv_utf16(b: &[u8], kind: Utf16) -> String {
    let odd = b.len() % 2 == 1;
    let (le, body) = match kind {
        Utf16::Le => (true, b),
        Utf16::Be => (false, b),
        Utf16::Bom if b.starts_with(&[0xFF, 0xFE]) => (true, &b[2..]),
        Utf16::Bom if b.starts_with(&[0xFE, 0xFF]) => (false, &b[2..]),
        Utf16::Bom => (!odd, b), // mb_convert_encoding defaults to big-endian
    };
    let units = body.as_chunks::<2>().0.iter().map(|&c| if le { u16::from_le_bytes(c) } else { u16::from_be_bytes(c) });
    let mut s: String = if odd {
        char::decode_utf16(units).map(|r| r.unwrap_or('?')).collect()
    } else {
        char::decode_utf16(units).filter_map(Result::ok).collect()
    };
    if odd {
        s.push('?');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn mp3_gbk_id3_and_cbr_duration() {
        let tag = id3v2_23(&[("TIT2", latin1_frame(&gbk("晴天"))), ("TPE1", latin1_frame(&gbk("周杰伦")))]);
        let p = write_temp("a.mp3", &[tag, mp3_cbr_frames(380)].concat());
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.artist.as_str(), i.codec.as_str(), i.bitrate), ("晴天", "周杰伦", "mp3", 128));
        assert!((i.duration - 380.0 * 1152.0 / 44100.0).abs() < 0.1);
    }

    #[test]
    fn tiny_apic_is_not_a_cover() {
        let tiny = [&[0xFFu8, 0xD8][..], &[0u8; 11]].concat();
        let p = write_temp("t.mp3", &[id3v2_23(&[("APIC", apic_frame(3, &tiny))]), mp3_cbr_frames(20)].concat());
        assert!(!read_audio_info(&p).pic);
        assert_eq!(read_cover(&p), None);

        let big = [&[0xFFu8, 0xD8, 0xFF, 0xE0][..], &[7u8; 196]].concat();
        let p = write_temp("b.mp3", &[id3v2_23(&[("APIC", apic_frame(3, &big))]), mp3_cbr_frames(20)].concat());
        assert!(read_audio_info(&p).pic);
        let (data, mime) = read_cover(&p).expect("cover");
        assert_eq!((data, mime), (big, "image/jpeg"));
    }

    #[test]
    fn front_cover_preferred() {
        let back = [&[0x89u8, b'P', b'N', b'G'][..], &[1u8; 200]].concat();
        let front = [&[0xFFu8, 0xD8, 0xFF, 0xE0][..], &[2u8; 200]].concat();
        let p = write_temp(
            "f.mp3",
            &[id3v2_23(&[("APIC", apic_frame(4, &back)), ("APIC", apic_frame(3, &front))]), mp3_cbr_frames(20)].concat(),
        );
        assert_eq!(read_cover(&p), Some((front, "image/jpeg")));
    }

    #[test]
    fn exe_renamed_to_mp3_is_notaudio() {
        let p = write_temp("x.mp3", &[&b"MZ\x90\0"[..], &[0u8; 1000]].concat());
        assert_eq!(read_audio_info(&p).codec, "notaudio");
    }

    #[test]
    fn wav_pcm_info() {
        let p = write_temp("w.wav", &wav_pcm(2.0, 44100, &[]));
        let i = read_audio_info(&p);
        assert_eq!((i.codec.as_str(), i.bitrate), ("pcm", 1411));
        assert!((i.duration - 2.0).abs() < 0.01);
    }

    #[test]
    fn wav_list_info_and_id3_chunk() {
        let mut list = b"INFO".to_vec();
        list.extend_from_slice(b"INAM\x06\0\0\0Hello\0");
        let p = write_temp(
            "w.wav",
            &wav_pcm(1.0, 8000, &[(b"LIST", list), (b"id3 ", id3v2_23(&[("TPE1", latin1_frame(b"World"))]))]),
        );
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.artist.as_str()), ("Hello", "World"));
    }

    #[test]
    fn flac_comments_and_picture() {
        let pic = [&[0xFFu8, 0xD8, 0xFF, 0xE0][..], &[3u8; 296]].concat();
        let p = write_temp("f.flac", &flac_file(&[("TITLE", "Hello"), ("ARTIST", "World")], Some((3, pic.clone()))));
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.artist.as_str(), i.codec.as_str(), i.pic), ("Hello", "World", "flac", true));
        assert!((i.duration - 10.0).abs() < 0.01);
        assert_eq!(read_cover(&p), Some((pic, "image/jpeg")));
    }

    #[test]
    fn id3v1_only_gbk() {
        let mut v1 = vec![0u8; 128];
        v1[..3].copy_from_slice(b"TAG");
        let t = gbk("晴天");
        v1[3..3 + t.len()].copy_from_slice(&t);
        v1[126] = 5;
        let p = write_temp("v1.mp3", &[mp3_cbr_frames(20), v1].concat());
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.track, i.codec.as_str()), ("晴天", 5, "mp3"));
    }

    #[test]
    fn apev2_tail_tag() {
        let mut items = Vec::new();
        for (k, v) in [("Title", "Hello"), ("Album Artist", "Band"), ("Track", "7/12")] {
            items.extend_from_slice(&(v.len() as u32).to_le_bytes());
            items.extend_from_slice(&0u32.to_le_bytes());
            items.extend_from_slice(k.as_bytes());
            items.push(0);
            items.extend_from_slice(v.as_bytes());
        }
        let mut foot = b"APETAGEX".to_vec();
        foot.extend_from_slice(&2000u32.to_le_bytes());
        foot.extend_from_slice(&(items.len() as u32 + 32).to_le_bytes());
        foot.extend_from_slice(&3u32.to_le_bytes());
        foot.extend_from_slice(&[0u8; 12]);
        let p = write_temp("ape.mp3", &[mp3_cbr_frames(20), items, foot].concat());
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.albumartist.as_str(), i.track), ("Hello", "Band", 7));
    }

    fn ape_items_and_footer(title: &str, claimed_size: u32) -> (Vec<u8>, Vec<u8>) {
        let mut items = Vec::new();
        items.extend_from_slice(&(title.len() as u32).to_le_bytes());
        items.extend_from_slice(&0u32.to_le_bytes());
        items.extend_from_slice(b"Title\0");
        items.extend_from_slice(title.as_bytes());
        let mut foot = b"APETAGEX".to_vec();
        foot.extend_from_slice(&2000u32.to_le_bytes());
        foot.extend_from_slice(&claimed_size.to_le_bytes());
        foot.extend_from_slice(&1u32.to_le_bytes());
        foot.extend_from_slice(&[0u8; 12]);
        (items, foot)
    }

    #[test]
    fn apev2_footer_larger_than_file_is_ignored() {
        // Footer claims ~4 GiB on a tiny file: must not read from offset 0 / the whole file.
        let (items, foot) = ape_items_and_footer("Bogus", 0xFFFF_FFF0);
        let p = write_temp("huge.mp3", &[items, vec![0u8; 300], foot].concat());
        let i = read_audio_info(&p);
        assert_eq!(i.title, "");
    }

    #[test]
    fn apev2_footer_over_16mib_is_ignored() {
        // Fits inside the file, but no real tag is this big: skipped without reading it.
        let claimed = (16u32 << 20) + 4096;
        let (items, foot) = ape_items_and_footer("Bogus", claimed);
        let pad = claimed as usize - 32 - items.len();
        let p = write_temp("big.mp3", &[items, vec![0u8; pad], foot].concat());
        assert_eq!(read_audio_info(&p).title, "");
    }

    #[test]
    fn truncated_id3_does_not_panic() {
        let mut b = b"ID3\x03\x00\x00\x00\x40\x00\x00".to_vec(); // declares 1 MiB
        b.extend_from_slice(b"TIT2\x00\x00\x00\x06\x00\x00\x00Hello");
        b.resize(200, 0xFF);
        let p = write_temp("tr.mp3", &b);
        let i = read_audio_info(&p);
        assert_eq!(i.title, "Hello");
        let _ = read_cover(&p);
    }

    #[test]
    fn garbage_files_do_not_panic() {
        // Every magic number followed by junk / truncation must return without panicking.
        let mut seed = 0x2545F491u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        };
        for magic in [&b"fLaC"[..], b"MAC ", b"RIFF", b"\x30\x26\xB2\x75", b"OggS", b"ID3\x04\x00\xC0", b"\0\0\0\x20ftyp"] {
            for len in [0usize, 3, 10, 40, 200, 3000] {
                let mut b = magic.to_vec();
                b.extend((0..len).map(|_| rnd()));
                for name in ["g.mp3", "g.m4a", "g.ogg"] {
                    let p = write_temp(name, &b);
                    let _ = read_audio_info(&p);
                    let _ = read_cover(&p);
                }
            }
        }
        assert_eq!(read_audio_info(Path::new("Z:/definitely/missing.mp3")), AudioInfo::default());
    }

    #[test]
    fn ogg_vorbis_comments_and_duration() {
        let mut id = b"\x01vorbis".to_vec();
        id.extend_from_slice(&0u32.to_le_bytes());
        id.push(2);
        id.extend_from_slice(&44100u32.to_le_bytes());
        id.extend_from_slice(&[0u8; 12]);
        id.extend_from_slice(&[0xB8, 1]);
        let pic = [&[0xFFu8, 0xD8, 0xFF, 0xE0][..], &[9u8; 296]].concat();
        let b64 = b64_encode(&flac_picture_block(3, "image/jpeg", &pic));
        let mut cm = b"\x03vorbis".to_vec();
        cm.extend_from_slice(&vorbis_comment_list(&[
            ("TITLE", "Hello"),
            ("ARTIST", "World"),
            ("TRACKNUMBER", "4"),
            ("METADATA_BLOCK_PICTURE", &b64),
        ]));
        cm.push(1);
        let p = write_temp("v.ogg", &ogg_stream(id, cm, 44100 * 30));
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.artist.as_str(), i.track, i.codec.as_str(), i.pic), ("Hello", "World", 4, "ogg", true));
        assert!((i.duration - 30.0).abs() < 0.01);
        let size = std::fs::metadata(&p).unwrap().len() as f64;
        assert_eq!(i.bitrate, (size * 8.0 / 30.0 / 1000.0).round() as u32); // size-based fallback
        assert_eq!(read_cover(&p), Some((pic, "image/jpeg")));
    }

    #[test]
    fn ogg_opus_duration() {
        let mut id = b"OpusHead\x01\x02".to_vec();
        id.extend_from_slice(&312u16.to_le_bytes());
        id.extend_from_slice(&44100u32.to_le_bytes());
        id.extend_from_slice(&[0, 0, 0]);
        let mut cm = b"OpusTags".to_vec();
        cm.extend_from_slice(&vorbis_comment_list(&[("title", "Opus song")]));
        let p = write_temp("o.opus", &ogg_stream(id, cm, 48000 * 12 + 312));
        let i = read_audio_info(&p);
        assert_eq!((i.title.as_str(), i.codec.as_str()), ("Opus song", "opus"));
        assert!((i.duration - 12.0).abs() < 0.01);
    }

    fn b64_encode(b: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                if k <= c.len() {
                    s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }
}
