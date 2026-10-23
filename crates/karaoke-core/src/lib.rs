//! Shared data model, SQLite store and pure lyric helpers for the karaoke app.

pub mod db;
pub mod lrc;
pub mod model;
pub mod pinyin;
pub mod timing;

/// True for CJK unified ideographs (including extension A, compatibility and the SIP planes).
pub fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}
