//! Works out whether "A - B" file names mean "artist - title" or "title - artist".
//!
//! The order is decided from the file's tags, the rest of the library and its folder.
//! Music files are never changed.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::text::{clean_tag, is_unknown};

/// How many decided songs a folder needs before it can overrule a song's own tags.
const FOLDER_MIN_VOTES: usize = 5;
const FOLDER_MIN_SHARE: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInput {
    pub root_id: i64,
    pub rel: String,
    pub tag_title: String,
    pub tag_artist: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    ArtistFirst,
    TitleFirst,
}

impl Order {
    fn flipped(self) -> Order {
        match self {
            Order::ArtistFirst => Order::TitleFirst,
            Order::TitleFirst => Order::ArtistFirst,
        }
    }
}

struct Parsed {
    folder: (i64, String),
    left: String,
    right: String,
    tight: bool,
    tag_artist: Option<String>,
    claim: Option<Order>,
}

impl Parsed {
    fn artist_side(&self, order: Order) -> &str {
        match order {
            Order::ArtistFirst => &self.left,
            Order::TitleFirst => &self.right,
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("valid regex"))
}

fn key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn same(a: &str, b: &str) -> bool {
    let (a, b) = (key(a), key(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || (a.chars().count().min(b.chars().count()) >= 4 && (a.contains(&b) || b.contains(&a)))
}

fn usable(tag: &str) -> Option<String> {
    static JUNK: OnceLock<Regex> = OnceLock::new();
    let t = clean_tag(tag);
    let t = t.trim();
    let junk = re(&JUNK, r"(?i)^(track\s*\d*|artist|title|www\..*|.*\.(com|cn|net|org)\b.*)$");
    (!t.is_empty() && !is_unknown(t) && !junk.is_match(t)).then(|| t.to_string())
}

fn artist_parts(s: &str) -> impl Iterator<Item = &str> {
    s.split(['/', ',', ';', '&', '、']).map(str::trim).filter(|p| !p.is_empty())
}

fn parse(n: &NameInput) -> Option<Parsed> {
    static NUM: OnceLock<Regex> = OnceLock::new();
    static SPACED: OnceLock<Regex> = OnceLock::new();
    static TIGHT: OnceLock<Regex> = OnceLock::new();
    let (dir, file) = n.rel.rsplit_once('/').unwrap_or(("", &n.rel));
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let plain = re(&NUM, r"^\d{1,3}[\s._-]+").replace(stem, "");
    let title = usable(&n.tag_title);
    let artist = usable(&n.tag_artist);
    let claim_for = |l: &str, r: &str| match (&title, &artist) {
        (Some(t), Some(a)) if same(l, a) && same(r, t) => Some(Order::ArtistFirst),
        (Some(t), Some(a)) if same(l, t) && same(r, a) => Some(Order::TitleFirst),
        _ => None,
    };
    let (left, right, tight) = if let Some(m) = re(&SPACED, r"^(.+?)\s+-\s+(.+)$").captures(&plain) {
        (m[1].trim().to_string(), m[2].trim().to_string(), false)
    } else {
        let m = re(&TIGHT, r"^(.+?)-(.+)$").captures(&plain)?;
        let (l, r) = (m[1].trim().to_string(), m[2].trim().to_string());
        // "Jay-Z" and similar names only split when the tags confirm both halves
        claim_for(&l, &r)?;
        (l, r, true)
    };
    if left.is_empty() || right.is_empty() || is_unknown(&left) || is_unknown(&right) {
        return None;
    }
    let claim = claim_for(&left, &right);
    Some(Parsed { folder: (n.root_id, dir.to_string()), left, right, tight, tag_artist: artist, claim })
}

fn known_artists(inputs: &[NameInput], parsed: &[Option<Parsed>]) -> HashMap<String, HashSet<usize>> {
    let mut seen: HashMap<String, HashSet<usize>> = HashMap::new();
    for (i, n) in inputs.iter().enumerate() {
        if let Some(a) = usable(&n.tag_artist) {
            for part in artist_parts(&a) {
                seen.entry(key(part)).or_default().insert(i);
            }
        }
        if let Some(p) = &parsed[i] {
            if let Some(order) = p.claim {
                seen.entry(key(p.artist_side(order))).or_default().insert(i);
            }
        }
    }
    seen.remove("");
    seen
}

fn output(p: &Parsed, order: Order) -> (String, String) {
    let (title, mut artist) = match order {
        Order::ArtistFirst => (p.right.clone(), p.left.clone()),
        Order::TitleFirst => (p.left.clone(), p.right.clone()),
    };
    if let Some(tag) = &p.tag_artist {
        // a file name cannot hold "/", so the tag has the full artist credit
        let single = artist_parts(&artist).count() == 1;
        let fuller = order == Order::TitleFirst && single && tag.contains('/') && artist_parts(tag).any(|x| same(x, &artist));
        if (p.tight && same(tag, &artist)) || fuller {
            artist = tag.clone();
        }
    }
    (title, artist)
}

/// The (title, artist) of each song named "A - B"; `None` for every other song.
pub fn resolve(inputs: &[NameInput]) -> Vec<Option<(String, String)>> {
    let parsed: Vec<Option<Parsed>> = inputs.iter().map(parse).collect();
    let artists = known_artists(inputs, &parsed);
    let others = |name: &str, i: usize| artists.get(&key(name)).map_or(0, |s| s.len() - usize::from(s.contains(&i)));

    let mut decided: Vec<Option<Order>> = vec![None; parsed.len()];
    let mut votes: HashMap<&(i64, String), (usize, usize)> = HashMap::new();
    for (i, p) in parsed.iter().enumerate() {
        let Some(p) = p else { continue };
        let Some(claim) = p.claim else { continue };
        let claimed = others(p.artist_side(claim), i);
        let opposite = others(p.artist_side(claim.flipped()), i);
        let order = if claimed >= 1 && claimed >= opposite {
            claim
        } else if opposite >= 2 && claimed == 0 {
            claim.flipped()
        } else {
            continue;
        };
        decided[i] = Some(order);
        let v = votes.entry(&p.folder).or_default();
        if order == Order::TitleFirst { v.0 += 1 } else { v.1 += 1 }
    }

    parsed
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let p = p.as_ref()?;
            let (title_votes, artist_votes) = votes.get(&p.folder).copied().unwrap_or((0, 0));
            let order = decided[i].unwrap_or_else(|| {
                let majority = if title_votes > artist_votes { Order::TitleFirst } else { Order::ArtistFirst };
                match p.claim {
                    // "A - B - C" has no clear pair, so it keeps the long-standing reading
                    None if p.right.contains(" - ") => Order::ArtistFirst,
                    None => majority,
                    Some(claim) => {
                        let total = title_votes + artist_votes;
                        let share = title_votes.max(artist_votes) as f64 / total.max(1) as f64;
                        if total >= FOLDER_MIN_VOTES && share >= FOLDER_MIN_SHARE { majority } else { claim }
                    }
                }
            });
            Some(output(p, order))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(rel: &str, title: &str, artist: &str) -> NameInput {
        NameInput { root_id: 1, rel: rel.into(), tag_title: title.into(), tag_artist: artist.into() }
    }
    fn pair(t: &str, a: &str) -> Option<(String, String)> {
        Some((t.into(), a.into()))
    }

    #[test]
    fn tags_decide_either_order() {
        let r = resolve(&[
            n("x/Beyond - Sea.mp3", "Sea", "Beyond"),
            n("x/Beyond - Sky.mp3", "Sky", "Beyond"),
            n("y/Shake It Off - Taylor Swift.mp3", "Shake It Off", "Taylor Swift"),
            n("z/Taylor Swift - Style.mp3", "Style", "Taylor Swift"),
        ]);
        assert_eq!(r[0], pair("Sea", "Beyond"));
        assert_eq!(r[2], pair("Shake It Off", "Taylor Swift"));
        assert_eq!(r[3], pair("Style", "Taylor Swift"));
    }

    #[test]
    fn a_swapped_tag_loses_to_the_rest_of_the_library() {
        let r = resolve(&[
            n("a/Avril Lavigne - Runaway.wma", "Avril Lavigne", "Runaway"),
            n("b/Avril Lavigne - Complicated.mp3", "Complicated", "Avril Lavigne"),
            n("c/Avril Lavigne - Innocence.mp3", "Innocence", "Avril Lavigne"),
        ]);
        assert_eq!(r[0], pair("Runaway", "Avril Lavigne"));
    }

    #[test]
    fn a_strong_folder_overrules_a_lone_swapped_tag() {
        let mut v: Vec<NameInput> = (0..6)
            .flat_map(|i| {
                let a = format!("Singer{i}");
                [n(&format!("k/Song{i} - {a}.mp3"), &format!("Song{i}"), &a), n(&format!("o/{a} - Other{i}.mp3"), &format!("Other{i}"), &a)]
            })
            .collect();
        v.retain(|x| x.rel.starts_with("k/"));
        v.extend((0..6).map(|i| n(&format!("o/Singer{i} - Other{i}.mp3"), &format!("Other{i}"), &format!("Singer{i}"))));
        v.push(n("k/半夏 - 徐颢菲.mp3", "徐颢菲", "半夏－"));
        assert_eq!(resolve(&v).last().unwrap(), &pair("半夏", "徐颢菲"));
    }

    #[test]
    fn untagged_songs_follow_their_folder() {
        let mut v: Vec<NameInput> = (0..3).map(|i| n(&format!("k/T{i}x - Singer.mp3"), &format!("T{i}x"), "Singer")).collect();
        v.push(n("k/海浪 - 黄品源.mp3", "Track 04", "www.example.cn"));
        v.push(n("other/黄品源 - 海浪.mp3", "", ""));
        let r = resolve(&v);
        assert_eq!(r[3], pair("海浪", "黄品源"));
        assert_eq!(r[4], pair("海浪", "黄品源"));
    }

    #[test]
    fn hyphens_inside_names_are_kept() {
        let r = resolve(&[
            n("Rihanna feat. Jay-Z - Umbrella.mp3", "", ""),
            n("Believer-ImagineDragons.mp3", "Believer", "Imagine Dragons"),
            n("Jay-Z.mp3", "Song", "Other"),
        ]);
        assert_eq!(r[0], pair("Umbrella", "Rihanna feat. Jay-Z"));
        assert_eq!(r[1], pair("Believer", "Imagine Dragons"));
        assert_eq!(r[2], None);
    }

    #[test]
    fn title_first_keeps_the_full_artist_credit() {
        let r = resolve(&[
            n("e/Baby - Justin Bieber.mp3", "Baby", "Justin Bieber/Ludacris"),
            n("f/Justin Bieber - Sorry.mp3", "Sorry", "Justin Bieber"),
        ]);
        assert_eq!(r[0], pair("Baby", "Justin Bieber/Ludacris"));
    }

    #[test]
    fn a_name_with_two_dashes_ignores_the_folder() {
        let mut v: Vec<NameInput> = (0..6).map(|i| n(&format!("k/Song{i} - Singer.mp3"), &format!("Song{i}"), "Singer")).collect();
        v.push(n("k/蔡淳佳 - 南屏晚钟 - 有一天我会.mp3", "", ""));
        assert_eq!(resolve(&v).last().unwrap(), &pair("南屏晚钟 - 有一天我会", "蔡淳佳"));
    }

    #[test]
    fn an_artist_list_in_the_file_name_is_kept() {
        let r = resolve(&[
            n("k/梦 - 青木月光,初音ミク.mp3", "梦", "青木月光/初音ミク"),
            n("o/初音ミク - 歌.mp3", "歌", "初音ミク"),
        ]);
        assert_eq!(r[0], pair("梦", "青木月光,初音ミク"));
    }

    #[test]
    fn names_without_a_pair_are_left_alone() {
        let r = resolve(&[n("281公里.mp3", "", ""), n("x/未知艺术家 - 未知标题.wav", "", ""), n("03 - 晴天.mp3", "晴天", "周杰伦")]);
        assert_eq!(r, vec![None, None, None]);
    }
}
