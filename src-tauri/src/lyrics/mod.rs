pub mod parse;

pub use parse::{merge_translation, parse_lrc, parse_yrc, Line, Parsed, Word};
pub mod local;
pub mod online;
pub mod lrclib;
pub mod netease;
