use super::{ParseError, SpPlaylist, Track};
use serde::Deserialize;

#[derive(Deserialize)]
struct Root {
    playlists: Vec<Pl>,
}
#[derive(Deserialize)]
struct Pl {
    name: String,
    #[serde(default)]
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    track: Option<Tk>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tk {
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: String,
}

pub fn parse_spotify_json(text: &str) -> Result<Vec<SpPlaylist>, ParseError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let root: Root = serde_json::from_str(text).map_err(|_| ParseError::BadJson)?;
    Ok(root
        .playlists
        .into_iter()
        .map(|p| SpPlaylist {
            name: p.name,
            tracks: p
                .items
                .into_iter()
                .filter_map(|i| i.track)
                .map(|t| Track {
                    title: t.track_name,
                    artists: if t.artist_name.trim().is_empty() { vec![] } else { vec![t.artist_name] },
                    album: t.album_name,
                    duration_ms: None,
                })
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::ParseError;

    #[test]
    fn json_playlists_skip_episodes() {
        let t = r#"{"playlists":[
          {"name":"P1","items":[{"track":{"trackName":"Yesterday","artistName":"The Beatles","albumName":"Help"}},{"track":null}]},
          {"name":"P2","items":[{"track":{"trackName":"晴天","artistName":"周杰伦","albumName":"叶惠美"}}]}]}"#;
        let v = parse_spotify_json(t).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].tracks.len(), 1);
        assert_eq!(v[1].tracks[0].artists, vec!["周杰伦"]);
        assert_eq!(parse_spotify_json("{\"foo\":1}"), Err(ParseError::BadJson));
    }
}
