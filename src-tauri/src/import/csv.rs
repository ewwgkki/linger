use super::{SpPlaylist, Track};
use super::ParseError;

/// RFC 4180 state machine. Returns rows of fields.
fn read_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_q = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_q = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => in_q = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn find_col(header: &[String], names: &[&str]) -> Option<usize> {
    names.iter().find_map(|n| header.iter().position(|h| h.trim().eq_ignore_ascii_case(n)))
}

pub fn parse_csv(name_hint: &str, text: &str) -> Result<SpPlaylist, ParseError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let rows = read_rows(text);
    let header = rows.first().ok_or(ParseError::NotCsv)?;
    let known = ["Track Name", "Artist Name(s)", "Artist Name", "Album Name", "Duration (ms)", "Track Duration (ms)"];
    if header.len() < 2 && find_col(header, &known).is_none() {
        return Err(ParseError::NotCsv);
    }
    let title_c = find_col(header, &["Track Name"]);
    let artist_c = find_col(header, &["Artist Name(s)", "Artist Name"]);
    let (Some(title_c), Some(artist_c)) = (title_c, artist_c) else {
        return Err(ParseError::MissingColumns);
    };
    let album_c = find_col(header, &["Album Name"]);
    let dur_c = find_col(header, &["Duration (ms)", "Track Duration (ms)"]);
    let get = |r: &Vec<String>, i: usize| r.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut tracks = Vec::new();
    for r in &rows[1..] {
        let title = get(r, title_c);
        if title.is_empty() {
            continue;
        }
        let artists = get(r, artist_c)
            .split([',', ';'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        tracks.push(Track {
            title,
            artists,
            album: album_c.map(|i| get(r, i)).unwrap_or_default(),
            duration_ms: dur_c.and_then(|i| get(r, i).parse::<u64>().ok()),
        });
    }
    if tracks.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(SpPlaylist { name: name_hint.to_string(), tracks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::ParseError;

    #[test]
    fn csv_quotes_newlines_bom() {
        let t = "\u{FEFF}Track Name,Artist Name(s),Album Name,Duration (ms)\r\n\"Hello, World\",\"A, B\",\"X \"\"Y\"\"\",1000\r\n\"Two\nLines\",C,D,2000\r\n";
        let p = parse_csv("n", t).unwrap();
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[0].title, "Hello, World");
        assert_eq!(p.tracks[0].artists, vec!["A", "B"]);
        assert_eq!(p.tracks[0].album, "X \"Y\"");
        assert_eq!(p.tracks[0].duration_ms, Some(1000));
        assert_eq!(p.tracks[1].title, "Two\nLines");
    }

    #[test]
    fn csv_header_by_name_any_order() {
        let t = "Album Name,Track Duration (ms),Artist Name,Track Name\nD,5,A;B,Hello\n";
        let p = parse_csv("n", t).unwrap();
        let tr = &p.tracks[0];
        assert_eq!((tr.title.as_str(), tr.album.as_str(), tr.duration_ms), ("Hello", "D", Some(5)));
        assert_eq!(tr.artists, vec!["A", "B"]);
    }

    #[test]
    fn csv_missing_columns() {
        assert_eq!(parse_csv("n", "Album Name\nx\n"), Err(ParseError::MissingColumns));
        assert_eq!(parse_csv("n", "hello"), Err(ParseError::NotCsv));
        assert_eq!(parse_csv("n", "Track Name,Artist Name\n"), Err(ParseError::Empty));
    }
}
