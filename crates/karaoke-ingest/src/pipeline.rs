//! `add`, `attach-audio` and `list` commands.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use jieba_rs::Jieba;
use karaoke_core::model::{Char, Line, NewSong, Word};
use karaoke_core::pinyin::{numbered_to_marked, numbered_to_marked_multi};
use karaoke_core::{db, lrc, timing};
use reqwest::blocking::Client;

use crate::AddArgs;
use crate::cedict::{self, Cedict};
use crate::llm::{self, LineInput, TokenAnalysis};
use crate::{lrclib, segment};

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("karaoke/0.1.0 (https://github.com/cgbarn35/karaoke)")
        .build()?)
}

/// Copies `src` to `<db_dir>/audio/<stem>.<ext>` and returns the DB-relative path.
fn copy_audio(db_path: &Path, src: &Path, stem: &str) -> Result<String> {
    if !src.is_file() {
        bail!("audio file {} does not exist", src.display());
    }
    let ext = src.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    let name = if ext.is_empty() { stem.to_string() } else { format!("{stem}.{ext}") };
    let dir = db::db_dir(db_path).join("audio");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::copy(src, dir.join(&name)).with_context(|| format!("copying {}", src.display()))?;
    Ok(format!("audio/{name}"))
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

pub fn add(db_path: &Path, a: &AddArgs) -> Result<()> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .context("ANTHROPIC_API_KEY is not set")?;
    if let Some(audio) = &a.audio
        && !audio.is_file()
    {
        bail!("audio file {} does not exist", audio.display());
    }
    let client = client()?;

    // 1. Lyrics.
    let (lrc_raw, lyrics_source, album, duration_hint, audio_stem) = match &a.lrc {
        Some(path) => {
            let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let base = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = path.file_stem().map(|f| sanitize(&f.to_string_lossy())).unwrap_or_default();
            (raw, format!("file:{base}"), None, a.duration_s, stem)
        }
        None => {
            let rec = match a.lrclib_id {
                Some(id) => lrclib::get(&client, id)?,
                None => lrclib::search(&client, &a.title, &a.artist, a.duration_s)?,
            };
            println!(
                "lrclib: using id={} album={} duration={}s",
                rec.id,
                rec.album_name.as_deref().unwrap_or(""),
                rec.duration.map(|d| d.to_string()).unwrap_or_default()
            );
            let id = rec.id;
            (rec.synced_lyrics.unwrap_or_default(), format!("lrclib:{id}"), rec.album_name, rec.duration, format!("lrclib-{id}"))
        }
    };
    let parsed = lrc::parse(&lrc_raw);
    if !parsed.iter().any(|l| !l.text.is_empty()) {
        bail!("no timed lyric lines in {lyrics_source}");
    }
    let duration_ms = match duration_hint {
        Some(d) => (d * 1000.0).round() as u32,
        None => parsed.last().map_or(0, |l| l.start_ms) + 10_000,
    };

    // 2-3. Timing and segmentation.
    let ends = timing::line_ends(&parsed, duration_ms);
    let jieba = Jieba::new();
    let inputs: Vec<LineInput> = parsed
        .iter()
        .map(|l| LineInput { text: l.text.clone(), tokens: segment::tokenize_line(&jieba, &l.text) })
        .collect();

    // 4-5. Dictionary + model.
    let cache = db::db_dir(db_path).join("cache");
    let cedict = Cedict::load(&cedict::ensure_downloaded(&client, &cache)?)?;
    let prompt = llm::build_prompt(&a.title, &a.artist, &inputs, &cedict);
    println!("anthropic: analysing {} lines with {}", inputs.iter().filter(|l| !l.text.is_empty()).count(), a.model);
    let (analysis, warnings) = llm::analyse(&client, &key, &a.model, &prompt, &inputs, &cedict)?;

    // 6. Assemble.
    let translations: HashMap<usize, &str> =
        analysis.lines.iter().map(|l| (l.index, l.translation.as_str())).collect();
    let tokens: HashMap<&str, &TokenAnalysis> =
        analysis.lines.iter().flat_map(|l| &l.tokens).map(|t| (t.id.as_str(), t)).collect();
    let lines: Vec<Line> = parsed
        .iter()
        .zip(&ends)
        .zip(&inputs)
        .enumerate()
        .map(|(i, ((l, &end_ms), input))| {
            let words = input
                .tokens
                .iter()
                .enumerate()
                .map(|(j, t)| build_word(t, tokens.get(llm::token_id(i, j).as_str()).copied(), &cedict))
                .collect();
            let mut line = Line {
                start_ms: l.start_ms,
                end_ms,
                text: l.text.clone(),
                translation: translations.get(&i).map(|s| s.to_string()).unwrap_or_default(),
                words,
            };
            timing::assign_char_times(&mut line);
            line
        })
        .collect();

    // 7. Audio.
    let audio_rel_path = a.audio.as_deref().map(|src| copy_audio(db_path, src, &audio_stem)).transpose()?;

    // 8. Store.
    let song = NewSong {
        title: a.title.clone(),
        title_reading: numbered_to_marked_multi(&analysis.title_reading),
        title_en: analysis.title_en.clone(),
        artist: a.artist.clone(),
        artist_reading: numbered_to_marked_multi(&analysis.artist_reading),
        artist_en: analysis.artist_en.clone(),
        language: "zh-Hans".into(),
        album,
        duration_ms,
        audio_rel_path,
        lyrics_source,
        lrc_raw,
        translation_model: a.model.clone(),
        lines,
    };
    let mut conn = db::open(db_path)?;
    let id = db::replace_song(&mut conn, &song)?;
    for w in &warnings {
        println!("warning: {w}");
    }
    println!("ingested song id={id}: {} — {} ({} lines)", song.title, song.artist, song.lines.len());
    Ok(())
}

fn build_word(t: &segment::Token, analysis: Option<&TokenAnalysis>, cedict: &Cedict) -> Word {
    match (t.is_han, analysis) {
        (true, Some(a)) => Word {
            text: t.text.clone(),
            is_han: true,
            reading: numbered_to_marked_multi(&a.reading),
            gloss: a.gloss.clone(),
            dict: cedict.dict_entries(&t.text),
            chars: a
                .chars
                .iter()
                .map(|c| Char {
                    ch: c.ch.clone(),
                    reading: numbered_to_marked(&c.reading),
                    gloss: c.gloss.clone(),
                    dict: cedict.dict_entries(&c.ch),
                    start_ms: 0,
                    end_ms: 0,
                })
                .collect(),
        },
        _ => Word {
            text: t.text.clone(),
            is_han: false,
            reading: String::new(),
            gloss: String::new(),
            dict: Vec::new(),
            chars: t
                .text
                .chars()
                .map(|c| Char {
                    ch: c.to_string(),
                    reading: String::new(),
                    gloss: String::new(),
                    dict: Vec::new(),
                    start_ms: 0,
                    end_ms: 0,
                })
                .collect(),
        },
    }
}

pub fn attach_audio(db_path: &Path, song_id: i64, audio: &Path) -> Result<()> {
    if !db_path.exists() {
        bail!("no database at {}", db_path.display());
    }
    let conn = db::open(db_path)?;
    if !db::list_songs(&conn)?.iter().any(|s| s.id == song_id) {
        bail!("no song with id {song_id}");
    }
    let rel = copy_audio(db_path, audio, &format!("song-{song_id}"))?;
    db::set_audio(&conn, song_id, &rel)?;
    println!("attached {rel} to song id={song_id}");
    Ok(())
}

pub fn list(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        bail!("no database at {}", db_path.display());
    }
    let conn = db::open(db_path)?;
    for s in db::list_songs(&conn)? {
        println!(
            "{}  {}  {}  {}  audio={}",
            s.id,
            s.title,
            s.artist,
            s.title_en,
            if s.has_audio { "yes" } else { "no" }
        );
    }
    Ok(())
}
