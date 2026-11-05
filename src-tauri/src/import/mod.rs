//! Parsing of Spotify exports (Exportify CSV/zip, "download your data" JSON)
//! and fuzzy matching against the library. Pure logic, no I/O.
use std::io::Read;

pub mod csv;
pub mod matcher;
pub mod spotify_json;

const MAX_ENTRY_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpPlaylist {
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    NotCsv,
    MissingColumns,
    BadJson,
    Empty,
    TooLarge,
    BadZip,
}

impl ParseError {
    pub fn key(&self) -> &'static str {
        match self {
            ParseError::NotCsv => "import.notCsv",
            ParseError::MissingColumns => "import.missingColumns",
            ParseError::BadJson => "import.badJson",
            ParseError::Empty => "import.empty",
            ParseError::TooLarge => "import.tooLarge",
            ParseError::BadZip => "import.badZip",
        }
    }
}

/// Strip a UTF-8 BOM; decode as UTF-8, falling back to GBK.
pub fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GBK.decode(bytes).0.into_owned(),
    }
}

/// File stem -> playlist name: '_' becomes ' ', trimmed; empty gives "Spotify".
pub fn playlist_name_from_file(stem: &str) -> String {
    let n = stem.replace('_', " ");
    let n = n.trim();
    if n.is_empty() { "Spotify".to_string() } else { n.to_string() }
}

pub fn parse_zip(bytes: &[u8]) -> Vec<(String, Result<SpPlaylist, ParseError>)> {
    let mut zip = match zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
        Ok(z) => z,
        Err(_) => return vec![(String::new(), Err(ParseError::BadZip))],
    };
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else { continue };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !name.to_lowercase().ends_with(".csv") {
            continue;
        }
        let mut buf = Vec::new();
        let res = match entry.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf) {
            Ok(_) if buf.len() as u64 > MAX_ENTRY_BYTES => Err(ParseError::TooLarge),
            Ok(_) => {
                let base = name.rsplit(['/', '\\']).next().unwrap_or(&name);
                let stem = base.get(..base.len() - 4).unwrap_or(base);
                csv::parse_csv(&playlist_name_from_file(stem), &decode_text(&buf))
            }
            Err(_) => Err(ParseError::BadZip),
        };
        out.push((name, res));
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn csv_gbk_resaved_by_excel() {
        let src = "Track Name,Artist Name(s),Album Name,Duration (ms)\r\n晴天,周杰伦,叶惠美,269000\r\n";
        let (bytes, _, _) = encoding_rs::GBK.encode(src);
        let text = decode_text(&bytes);
        let p = csv::parse_csv("x", &text).unwrap();
        assert_eq!(p.tracks[0].title, "晴天");
        assert_eq!(p.tracks[0].artists, vec!["周杰伦"]);
    }

    #[test]
    fn zip_reads_csv_entries_only() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("a_b.csv", o).unwrap();
            w.write_all(b"Track Name,Artist Name\nYesterday,The Beatles\n").unwrap();
            w.start_file("notes.txt", o).unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        let r = parse_zip(&buf);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "a_b.csv");
        assert_eq!(r[0].1.as_ref().unwrap().tracks.len(), 1);
        assert_eq!(playlist_name_from_file("a_b"), "a b");
        assert_eq!(playlist_name_from_file("  "), "Spotify");
    }

    #[test]
    fn zip_garbage_is_badzip() {
        let r = parse_zip(b"not a zip at all");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "");
        assert_eq!(r[0].1, Err(ParseError::BadZip));
    }
}
