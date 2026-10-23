//! Song analysis via the Anthropic Messages API (forced tool use) and its validation.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cedict::Cedict;
use crate::segment::Token;

const TOOL_NAME: &str = "record_song_analysis";

#[derive(Clone, Debug, Deserialize)]
pub struct SongAnalysis {
    pub title_reading: String,
    pub title_en: String,
    pub artist_reading: String,
    pub artist_en: String,
    pub lines: Vec<LineAnalysis>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LineAnalysis {
    pub index: usize,
    pub translation: String,
    pub tokens: Vec<TokenAnalysis>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TokenAnalysis {
    pub id: String,
    /// Numbered pinyin, one syllable per char.
    pub reading: String,
    pub gloss: String,
    pub chars: Vec<CharAnalysis>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CharAnalysis {
    pub ch: String,
    pub reading: String,
    pub gloss: String,
}

/// One LRC line with its tokens (empty `text` = instrumental break).
pub struct LineInput {
    pub text: String,
    pub tokens: Vec<Token>,
}

pub fn token_id(line: usize, word: usize) -> String {
    format!("{line}.{word}")
}

fn input_schema() -> Value {
    let s = || json!({"type": "string"});
    json!({
        "type": "object",
        "required": ["title_reading", "title_en", "artist_reading", "artist_en", "lines"],
        "properties": {
            "title_reading": s(), "title_en": s(), "artist_reading": s(), "artist_en": s(),
            "lines": {"type": "array", "items": {
                "type": "object", "required": ["index", "translation", "tokens"],
                "properties": {
                    "index": {"type": "integer"}, "translation": s(),
                    "tokens": {"type": "array", "items": {
                        "type": "object", "required": ["id", "reading", "gloss", "chars"],
                        "properties": {
                            "id": s(), "reading": s(), "gloss": s(),
                            "chars": {"type": "array", "items": {
                                "type": "object", "required": ["ch", "reading", "gloss"],
                                "properties": {"ch": s(), "reading": s(), "gloss": s()}
                            }}
                        }
                    }}
                }
            }}
        }
    })
}

fn candidates(cedict: &Cedict, key: &str) -> Value {
    Value::Array(
        cedict
            .get(key)
            .iter()
            .map(|e| json!({"reading": e.numbered, "defs": e.dict.defs.iter().take(3).collect::<Vec<_>>()}))
            .collect(),
    )
}

pub fn build_prompt(title: &str, artist: &str, lines: &[LineInput], cedict: &Cedict) -> String {
    let mut p = format!(
        "You are annotating the Mandarin song \"{title}\" by \"{artist}\" for a karaoke learner.\n\n\
         Lyric lines (index: text):\n"
    );
    for (i, l) in lines.iter().enumerate().filter(|(_, l)| !l.text.is_empty()) {
        p.push_str(&format!("L{i}: {}\n", l.text));
    }
    p.push_str(
        "\nHan tokens to annotate (one JSON object per token; candidates come from CC-CEDICT, \
         readings are numbered pinyin):\n",
    );
    for (i, l) in lines.iter().enumerate() {
        for (j, t) in l.tokens.iter().enumerate().filter(|(_, t)| t.is_han) {
            let mut chars = serde_json::Map::new();
            for c in t.text.chars() {
                let c = c.to_string();
                let v = candidates(cedict, &c);
                chars.insert(c, v);
            }
            let obj = json!({
                "id": token_id(i, j),
                "text": t.text,
                "word_candidates": candidates(cedict, &t.text),
                "char_candidates": chars,
            });
            p.push_str(&obj.to_string());
            p.push('\n');
        }
    }
    p.push_str(
        "\nCall the record_song_analysis tool with:\n\
         - title_reading / artist_reading: numbered pinyin of the title and artist name; \
           title_en: the English title; artist_en: the artist's usual English name.\n\
         - lines: one entry for EVERY lyric line listed above (index = the number after L), with \
           `translation`: a natural, faithful English translation that keeps the poetic sense, and \
           `tokens`: one entry for EVERY Han token of that line, using its exact id.\n\
         - For each token: `reading` = the contextual reading as numbered pinyin, one syllable per \
           character, space-separated, tone 5 for neutral tone; use citation tones (no tone sandhi). \
           `gloss` = the meaning of the word in this line, at most 6 words. \
           `chars` = one entry per character in order, with `ch` (the character), `reading` (its \
           numbered syllable, matching the token reading) and `gloss` (the meaning of that single \
           character as used here, at most 4 words).\n\
         Prefer readings from the candidates when one fits the context.\n",
    );
    p
}

/// Sends the request and returns the forced tool call's raw `input`.
fn call_once(client: &Client, key: &str, model: &str, prompt: &str) -> Result<Value> {
    let body = json!({
        "model": model,
        "max_tokens": 16000,
        "tools": [{
            "name": TOOL_NAME,
            "description": "Record the reading, glosses and translation of every line and token of the song.",
            "input_schema": input_schema(),
        }],
        "tool_choice": {"type": "tool", "name": TOOL_NAME},
        "messages": [{"role": "user", "content": prompt}],
    });
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(300))
        .json(&body)
        .send()?;
    let status = resp.status();
    let text = resp.text()?;
    if !status.is_success() {
        bail!("anthropic: HTTP {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("anthropic: invalid JSON response")?;
    if v["stop_reason"] == "max_tokens" {
        bail!("anthropic: response truncated");
    }
    v["content"]
        .as_array()
        .and_then(|c| c.iter().find(|b| b["type"] == "tool_use"))
        .map(|b| b["input"].clone())
        .ok_or_else(|| anyhow!("anthropic: no tool_use block in response"))
}

/// Calls the model and validates the result, retrying once if the output is malformed or invalid.
pub fn analyse(
    client: &Client,
    key: &str,
    model: &str,
    prompt: &str,
    lines: &[LineInput],
    cedict: &Cedict,
) -> Result<(SongAnalysis, Vec<String>)> {
    // Outer Err = transport/HTTP failure (not retried); inner Err = model drift (retried).
    let attempt = || -> Result<Result<(SongAnalysis, Vec<String>)>> {
        let input = call_once(client, key, model, prompt)?;
        Ok(serde_json::from_value::<SongAnalysis>(input)
            .context("tool input does not match the schema")
            .and_then(|a| validate(a, lines, cedict)))
    };
    match attempt()? {
        Ok(r) => Ok(r),
        Err(e) => {
            eprintln!("analysis invalid ({e:#}); retrying once");
            attempt()?.context("analysis invalid after retry")
        }
    }
}

/// `lu:4`-style lowercase normalisation for comparing numbered pinyin.
fn normalize(r: &str) -> String {
    r.trim().to_lowercase().replace('v', "u:")
}

fn base(syl: &str) -> &str {
    syl.trim_end_matches(|c: char| c.is_ascii_digit())
}

/// Checks the analysis covers every lyric line and Han token exactly once with matching chars,
/// replaces char readings CEDICT does not know with CEDICT's first reading, and rebuilds token
/// readings from the char readings.
pub fn validate(
    mut analysis: SongAnalysis,
    lines: &[LineInput],
    cedict: &Cedict,
) -> Result<(SongAnalysis, Vec<String>)> {
    let mut warnings = Vec::new();

    let mut seen_lines = HashSet::new();
    for l in &analysis.lines {
        if lines.get(l.index).is_none_or(|x| x.text.is_empty()) {
            bail!("line L{} is not a lyric line", l.index);
        }
        if !seen_lines.insert(l.index) {
            bail!("line L{} appears more than once", l.index);
        }
    }
    if let Some((i, _)) = lines.iter().enumerate().find(|(i, l)| !l.text.is_empty() && !seen_lines.contains(i)) {
        bail!("line L{i} is missing");
    }

    let expected: HashMap<String, &Token> = lines
        .iter()
        .enumerate()
        .flat_map(|(i, l)| l.tokens.iter().enumerate().filter(|(_, t)| t.is_han).map(move |(j, t)| (token_id(i, j), t)))
        .collect();
    let mut seen_tokens = HashSet::new();
    for line in &mut analysis.lines {
        line.tokens.retain(|t| {
            let known = expected.contains_key(&t.id);
            if !known {
                warnings.push(format!("ignoring unknown token id {}", t.id));
            }
            known
        });
        for tok in &mut line.tokens {
            if !seen_tokens.insert(tok.id.clone()) {
                bail!("token {} appears more than once", tok.id);
            }
            let want = expected[&tok.id];
            let got: String = tok.chars.iter().map(|c| c.ch.as_str()).collect();
            if got != want.text || tok.chars.len() != want.text.chars().count() {
                bail!("token {} chars {:?} do not match {:?}", tok.id, got, want.text);
            }
            let word_syllables: Vec<Vec<String>> = cedict
                .get(&want.text)
                .iter()
                .map(|e| e.numbered.split_whitespace().map(normalize).collect())
                .collect();
            let n = tok.chars.len();
            for (k, c) in tok.chars.iter_mut().enumerate() {
                let entries = cedict.get(&c.ch);
                if entries.is_empty() {
                    continue;
                }
                let r = normalize(&c.reading);
                let known = entries.iter().any(|e| normalize(&e.numbered) == r)
                    // The word's own CEDICT reading (e.g. neutral tone in 月亮 yue4 liang5).
                    || word_syllables.iter().any(|s| s.len() == n && s[k] == r)
                    // Neutral tone of a known syllable.
                    || (r.ends_with('5') && entries.iter().any(|e| base(&normalize(&e.numbered)) == base(&r)));
                if !known {
                    let r0 = normalize(&entries[0].numbered);
                    warnings.push(format!(
                        "reading {} for {} in token {} not in CEDICT; using {r0}",
                        c.reading, c.ch, tok.id
                    ));
                    c.reading = r0;
                } else {
                    c.reading = r;
                }
            }
            tok.reading = tok.chars.iter().map(|c| c.reading.as_str()).collect::<Vec<_>>().join(" ");
        }
    }
    let mut missing: Vec<&String> = expected.keys().filter(|id| !seen_tokens.contains(*id)).collect();
    missing.sort();
    if let Some(id) = missing.first() {
        bail!("token {id} is missing");
    }
    Ok((analysis, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cedict::fixture;

    fn inputs() -> Vec<LineInput> {
        let t = |s: &str, h| Token { text: s.into(), is_han: h };
        vec![
            LineInput { text: String::new(), tokens: vec![] },
            LineInput { text: "月亮 的".into(), tokens: vec![t("月亮", true), t(" ", false), t("的", true)] },
        ]
    }

    fn analysis(de: &str) -> SongAnalysis {
        let c = |ch: &str, r: &str| CharAnalysis { ch: ch.into(), reading: r.into(), gloss: "g".into() };
        SongAnalysis {
            title_reading: String::new(),
            title_en: String::new(),
            artist_reading: String::new(),
            artist_en: String::new(),
            lines: vec![LineAnalysis {
                index: 1,
                translation: "the moon's".into(),
                tokens: vec![
                    TokenAnalysis {
                        id: "1.0".into(),
                        reading: "x".into(),
                        gloss: "moon".into(),
                        chars: vec![c("月", "yue4"), c("亮", "liang5")],
                    },
                    TokenAnalysis { id: "1.2".into(), reading: de.into(), gloss: "of".into(), chars: vec![c("的", de)] },
                ],
            }],
        }
    }

    #[test]
    fn accepts_and_rebuilds_readings() {
        let (a, warnings) = validate(analysis("de5"), &inputs(), &fixture()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        // 亮 is liang4 in CEDICT but liang5 inside 月亮; 月 is not in the fixture.
        assert_eq!(a.lines[0].tokens[0].reading, "yue4 liang5");
    }

    #[test]
    fn missing_token_is_error() {
        let mut a = analysis("de5");
        a.lines[0].tokens.pop();
        let err = validate(a, &inputs(), &fixture()).unwrap_err().to_string();
        assert!(err.contains("1.2"), "{err}");
    }

    #[test]
    fn char_mismatch_is_error() {
        let mut a = analysis("de5");
        a.lines[0].tokens[0].chars[1].ch = "光".into();
        let err = validate(a, &inputs(), &fixture()).unwrap_err().to_string();
        assert!(err.contains("1.0"), "{err}");
    }

    #[test]
    fn missing_line_is_error() {
        let mut a = analysis("de5");
        a.lines[0].index = 0;
        assert!(validate(a, &inputs(), &fixture()).is_err());
    }

    #[test]
    fn unknown_reading_falls_back_to_cedict() {
        let (a, warnings) = validate(analysis("di4"), &inputs(), &fixture()).unwrap();
        assert_eq!(a.lines[0].tokens[1].chars[0].reading, "de5");
        assert_eq!(a.lines[0].tokens[1].reading, "de5");
        assert_eq!(warnings, vec!["reading di4 for 的 in token 1.2 not in CEDICT; using de5"]);
    }
}
