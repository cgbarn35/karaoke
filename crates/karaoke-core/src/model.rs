use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DictEntry {
    pub traditional: String,
    pub simplified: String,
    /// Tone-marked reading, e.g. `yuè liang`.
    pub reading: String,
    pub defs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Char {
    pub ch: String,
    pub reading: String,
    pub gloss: String,
    pub dict: Vec<DictEntry>,
    pub start_ms: u32,
    pub end_ms: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub is_han: bool,
    pub reading: String,
    pub gloss: String,
    pub dict: Vec<DictEntry>,
    pub chars: Vec<Char>,
}

/// One timed lyric line. Empty `text` marks an instrumental break.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
    pub translation: String,
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SongSummary {
    pub id: i64,
    pub title: String,
    pub title_reading: String,
    pub title_en: String,
    pub artist: String,
    pub artist_reading: String,
    pub artist_en: String,
    pub has_audio: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub summary: SongSummary,
    pub duration_ms: u32,
    /// Absolute path, resolved against the DB directory.
    pub audio_path: Option<PathBuf>,
    pub offset_ms: i32,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewSong {
    pub title: String,
    pub title_reading: String,
    pub title_en: String,
    pub artist: String,
    pub artist_reading: String,
    pub artist_en: String,
    pub language: String,
    pub album: Option<String>,
    pub duration_ms: u32,
    /// Relative to the DB directory; `None` keeps any existing audio on re-ingest.
    pub audio_rel_path: Option<String>,
    pub lyrics_source: String,
    pub lrc_raw: String,
    pub translation_model: String,
    pub lines: Vec<Line>,
}
