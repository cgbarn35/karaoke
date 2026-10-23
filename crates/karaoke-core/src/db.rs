use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Char, DictEntry, Line, NewSong, Song, SongSummary, Word};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE artists(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, name_reading TEXT NOT NULL, name_en TEXT NOT NULL);
CREATE TABLE songs(
  id INTEGER PRIMARY KEY,
  artist_id INTEGER NOT NULL REFERENCES artists(id),
  title TEXT NOT NULL, title_reading TEXT NOT NULL, title_en TEXT NOT NULL,
  language TEXT NOT NULL,
  album TEXT,
  duration_ms INTEGER NOT NULL,
  audio_path TEXT,
  offset_ms INTEGER NOT NULL DEFAULT 0,
  lyrics_source TEXT NOT NULL,
  lrc_raw TEXT NOT NULL,
  translation_model TEXT NOT NULL,
  ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  UNIQUE(artist_id, title));
CREATE TABLE lines(id INTEGER PRIMARY KEY, song_id INTEGER NOT NULL REFERENCES songs(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
  text TEXT NOT NULL, translation TEXT NOT NULL, UNIQUE(song_id, idx));
CREATE TABLE words(id INTEGER PRIMARY KEY, line_id INTEGER NOT NULL REFERENCES lines(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL, text TEXT NOT NULL, is_han INTEGER NOT NULL,
  reading TEXT NOT NULL, gloss TEXT NOT NULL, dict_json TEXT NOT NULL, UNIQUE(line_id, idx));
CREATE TABLE chars(id INTEGER PRIMARY KEY, word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL, ch TEXT NOT NULL, reading TEXT NOT NULL, gloss TEXT NOT NULL,
  dict_json TEXT NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL, UNIQUE(word_id, idx));
";

/// Opens (creating if needed) the database and migrates an empty file to the current schema.
pub fn open(path: &Path) -> Result<Connection> {
    let dir = db_dir(path);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    init(conn)
}

fn init(mut conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        bail!("database schema version {v} is newer than this build supports ({SCHEMA_VERSION})");
    }
    if v == 0 {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
    }
    Ok(conn)
}

/// Directory containing the DB file (`.` when the path has no parent).
pub fn db_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Inserts or atomically replaces the song identified by (artist, title).
/// A replaced song keeps its `offset_ms`, and its `audio_path` unless a new one is given.
pub fn replace_song(conn: &mut Connection, s: &NewSong) -> Result<i64> {
    let tx = conn.transaction()?;
    let artist_id: i64 = tx.query_row(
        "INSERT INTO artists(name, name_reading, name_en) VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET name_reading=excluded.name_reading, name_en=excluded.name_en
         RETURNING id",
        params![s.artist, s.artist_reading, s.artist_en],
        |r| r.get(0),
    )?;
    let prev: Option<(i64, i32, Option<String>)> = tx
        .query_row(
            "SELECT id, offset_ms, audio_path FROM songs WHERE artist_id=?1 AND title=?2",
            params![artist_id, s.title],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (offset_ms, prev_audio) = match prev {
        Some((id, off, audio)) => {
            tx.execute("DELETE FROM songs WHERE id=?1", [id])?;
            (off, audio)
        }
        None => (0, None),
    };
    let audio = s.audio_rel_path.clone().or(prev_audio);
    tx.execute(
        "INSERT INTO songs(artist_id, title, title_reading, title_en, language, album, duration_ms,
                           audio_path, offset_ms, lyrics_source, lrc_raw, translation_model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            artist_id, s.title, s.title_reading, s.title_en, s.language, s.album, s.duration_ms,
            audio, offset_ms, s.lyrics_source, s.lrc_raw, s.translation_model
        ],
    )?;
    let song_id = tx.last_insert_rowid();
    {
        let mut ins_line = tx.prepare(
            "INSERT INTO lines(song_id, idx, start_ms, end_ms, text, translation) VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        let mut ins_word = tx.prepare(
            "INSERT INTO words(line_id, idx, text, is_han, reading, gloss, dict_json) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )?;
        let mut ins_char = tx.prepare(
            "INSERT INTO chars(word_id, idx, ch, reading, gloss, dict_json, start_ms, end_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )?;
        for (li, line) in s.lines.iter().enumerate() {
            ins_line.execute(params![song_id, li as i64, line.start_ms, line.end_ms, line.text, line.translation])?;
            let line_id = tx.last_insert_rowid();
            for (wi, w) in line.words.iter().enumerate() {
                ins_word.execute(params![
                    line_id, wi as i64, w.text, w.is_han, w.reading, w.gloss, serde_json::to_string(&w.dict)?
                ])?;
                let word_id = tx.last_insert_rowid();
                for (ci, c) in w.chars.iter().enumerate() {
                    ins_char.execute(params![
                        word_id, ci as i64, c.ch, c.reading, c.gloss, serde_json::to_string(&c.dict)?,
                        c.start_ms, c.end_ms
                    ])?;
                }
            }
        }
    }
    tx.commit()?;
    Ok(song_id)
}

const SUMMARY_COLS: &str = "s.id, s.title, s.title_reading, s.title_en, a.name, a.name_reading, a.name_en, s.audio_path";

fn summary_from_row(r: &rusqlite::Row) -> rusqlite::Result<SongSummary> {
    Ok(SongSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        title_reading: r.get(2)?,
        title_en: r.get(3)?,
        artist: r.get(4)?,
        artist_reading: r.get(5)?,
        artist_en: r.get(6)?,
        has_audio: r.get::<_, Option<String>>(7)?.is_some(),
    })
}

/// All songs, most recently ingested first.
pub fn list_songs(conn: &Connection) -> Result<Vec<SongSummary>> {
    let mut st = conn.prepare(&format!(
        "SELECT {SUMMARY_COLS} FROM songs s JOIN artists a ON a.id = s.artist_id
         ORDER BY s.ingested_at DESC, s.id DESC"
    ))?;
    let rows = st.query_map([], summary_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn parse_dict(json: &str) -> rusqlite::Result<Vec<DictEntry>> {
    serde_json::from_str(json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

/// Loads a full song (lines, words, chars) with four queries.
pub fn load_song(conn: &Connection, id: i64, db_dir: &Path) -> Result<Song> {
    let (summary, duration_ms, audio, offset_ms) = conn
        .query_row(
            &format!(
                "SELECT {SUMMARY_COLS}, s.duration_ms, s.offset_ms FROM songs s
                 JOIN artists a ON a.id = s.artist_id WHERE s.id = ?1"
            ),
            [id],
            |r| Ok((summary_from_row(r)?, r.get::<_, u32>(8)?, r.get::<_, Option<String>>(7)?, r.get::<_, i32>(9)?)),
        )
        .optional()?
        .with_context(|| format!("no song with id {id}"))?;

    let mut lines: Vec<Line> = Vec::new();
    let mut line_pos: HashMap<i64, usize> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT id, start_ms, end_ms, text, translation FROM lines WHERE song_id=?1 ORDER BY idx",
        )?;
        let mut rows = st.query([id])?;
        while let Some(r) = rows.next()? {
            line_pos.insert(r.get(0)?, lines.len());
            lines.push(Line {
                start_ms: r.get(1)?,
                end_ms: r.get(2)?,
                text: r.get(3)?,
                translation: r.get(4)?,
                words: Vec::new(),
            });
        }
    }
    // word id -> (line position, word position)
    let mut word_pos: HashMap<i64, (usize, usize)> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT w.id, w.line_id, w.text, w.is_han, w.reading, w.gloss, w.dict_json
             FROM words w JOIN lines l ON l.id = w.line_id WHERE l.song_id=?1 ORDER BY w.line_id, w.idx",
        )?;
        let mut rows = st.query([id])?;
        while let Some(r) = rows.next()? {
            let li = line_pos[&r.get::<_, i64>(1)?];
            let words = &mut lines[li].words;
            word_pos.insert(r.get(0)?, (li, words.len()));
            words.push(Word {
                text: r.get(2)?,
                is_han: r.get(3)?,
                reading: r.get(4)?,
                gloss: r.get(5)?,
                dict: parse_dict(&r.get::<_, String>(6)?)?,
                chars: Vec::new(),
            });
        }
    }
    {
        let mut st = conn.prepare(
            "SELECT c.word_id, c.ch, c.reading, c.gloss, c.dict_json, c.start_ms, c.end_ms
             FROM chars c JOIN words w ON w.id = c.word_id JOIN lines l ON l.id = w.line_id
             WHERE l.song_id=?1 ORDER BY c.word_id, c.idx",
        )?;
        let mut rows = st.query([id])?;
        while let Some(r) = rows.next()? {
            let (li, wi) = word_pos[&r.get::<_, i64>(0)?];
            lines[li].words[wi].chars.push(Char {
                ch: r.get(1)?,
                reading: r.get(2)?,
                gloss: r.get(3)?,
                dict: parse_dict(&r.get::<_, String>(4)?)?,
                start_ms: r.get(5)?,
                end_ms: r.get(6)?,
            });
        }
    }
    Ok(Song {
        summary,
        duration_ms,
        audio_path: audio.map(|rel| db_dir.join(rel)),
        offset_ms,
        lines,
    })
}

fn update_one(conn: &Connection, sql: &str, p: impl rusqlite::Params, id: i64) -> Result<()> {
    if conn.execute(sql, p)? == 0 {
        bail!("no song with id {id}");
    }
    Ok(())
}

pub fn set_offset(conn: &Connection, id: i64, ms: i32) -> Result<()> {
    update_one(conn, "UPDATE songs SET offset_ms=?1 WHERE id=?2", params![ms, id], id)
}

pub fn set_audio(conn: &Connection, id: i64, rel: &str) -> Result<()> {
    update_one(conn, "UPDATE songs SET audio_path=?1 WHERE id=?2", params![rel, id], id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn entry(s: &str, r: &str) -> DictEntry {
        DictEntry { traditional: s.into(), simplified: s.into(), reading: r.into(), defs: vec!["d".into()] }
    }

    fn sample(audio: Option<&str>) -> NewSong {
        let ch = |c: &str, r: &str, s, e| Char {
            ch: c.into(), reading: r.into(), gloss: format!("g-{c}"), dict: vec![entry(c, r)], start_ms: s, end_ms: e,
        };
        NewSong {
            title: "你好".into(),
            title_reading: "nǐ hǎo".into(),
            title_en: "Hello".into(),
            artist: "某人".into(),
            artist_reading: "mǒu rén".into(),
            artist_en: "Someone".into(),
            language: "zh-Hans".into(),
            album: Some("A".into()),
            duration_ms: 9000,
            audio_rel_path: audio.map(Into::into),
            lyrics_source: "file:x.lrc".into(),
            lrc_raw: "[00:01.00]你好 世界".into(),
            translation_model: "m".into(),
            lines: vec![
                Line {
                    start_ms: 1000,
                    end_ms: 2000,
                    text: "你好 世界".into(),
                    translation: "Hello world".into(),
                    words: vec![
                        Word {
                            text: "你好".into(), is_han: true, reading: "nǐ hǎo".into(), gloss: "hello".into(),
                            dict: vec![entry("你好", "nǐ hǎo")],
                            chars: vec![ch("你", "nǐ", 1000, 1250), ch("好", "hǎo", 1250, 1500)],
                        },
                        Word {
                            text: " ".into(), is_han: false, reading: String::new(), gloss: String::new(),
                            dict: vec![], chars: vec![ch(" ", "", 1500, 1500)],
                        },
                        Word {
                            text: "世界".into(), is_han: true, reading: "shì jiè".into(), gloss: "world".into(),
                            dict: vec![],
                            chars: vec![ch("世", "shì", 1500, 1750), ch("界", "jiè", 1750, 2000)],
                        },
                    ],
                },
                Line { start_ms: 2000, end_ms: 3000, text: String::new(), translation: String::new(), words: vec![] },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let mut c = mem();
        let s = sample(Some("audio/a.mp3"));
        let id = replace_song(&mut c, &s).unwrap();
        let got = load_song(&c, id, Path::new("/db")).unwrap();
        assert_eq!(got.lines, s.lines);
        assert_eq!(got.duration_ms, 9000);
        assert_eq!(got.offset_ms, 0);
        assert_eq!(got.audio_path, Some(PathBuf::from("/db/audio/a.mp3")));
        let sum = &got.summary;
        assert_eq!(
            (sum.title.as_str(), sum.title_reading.as_str(), sum.title_en.as_str()),
            ("你好", "nǐ hǎo", "Hello")
        );
        assert_eq!(
            (sum.artist.as_str(), sum.artist_reading.as_str(), sum.artist_en.as_str(), sum.has_audio),
            ("某人", "mǒu rén", "Someone", true)
        );
        assert_eq!(list_songs(&c).unwrap(), vec![sum.clone()]);
    }

    #[test]
    fn reingest_keeps_offset_and_audio() {
        let mut c = mem();
        let id = replace_song(&mut c, &sample(Some("audio/a.mp3"))).unwrap();
        set_offset(&c, id, 300).unwrap();
        let mut again = sample(None);
        again.title_en = "Hi".into();
        let id2 = replace_song(&mut c, &again).unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM songs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let got = load_song(&c, id2, Path::new("d")).unwrap();
        assert_eq!(got.offset_ms, 300);
        assert_eq!(got.summary.title_en, "Hi");
        assert_eq!(got.audio_path, Some(PathBuf::from("d/audio/a.mp3")));
        // Old line/word/char rows were cascaded away.
        let chars: i64 = c.query_row("SELECT count(*) FROM chars", [], |r| r.get(0)).unwrap();
        assert_eq!(chars, 5);
    }

    #[test]
    fn newer_schema_rejected() {
        let c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "user_version", 2).unwrap();
        let err = init(c).unwrap_err().to_string();
        assert_eq!(err, "database schema version 2 is newer than this build supports (1)");
    }

    #[test]
    fn set_offset_unknown_id() {
        let c = mem();
        assert_eq!(set_offset(&c, 42, 1).unwrap_err().to_string(), "no song with id 42");
    }
}
