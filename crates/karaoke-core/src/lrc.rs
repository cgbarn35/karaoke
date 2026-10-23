//! Minimal LRC parser: line-level timestamps only.

#[derive(Clone, Debug, PartialEq)]
pub struct LrcLine {
    pub start_ms: u32,
    pub text: String,
}

const CREDIT_PREFIXES: [&str; 10] =
    ["作词", "作曲", "编曲", "词", "曲", "制作人", "监制", "Lyrics", "Composer", "Arranger"];

/// Parses `mm:ss`, `mm:ss.x`, `mm:ss.xx`, `mm:ss.xxx` into milliseconds.
fn parse_time(tag: &str) -> Option<u32> {
    let (mm, rest) = tag.split_once(':')?;
    let (ss, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(mm) || !digits(ss) || !(frac.is_empty() || digits(frac) && frac.len() <= 3) {
        return None;
    }
    let frac_ms = if frac.is_empty() { 0 } else { frac.parse::<u32>().ok()? * 10u32.pow(3 - frac.len() as u32) };
    Some(mm.parse::<u32>().ok()? * 60_000 + ss.parse::<u32>().ok()? * 1000 + frac_ms)
}

fn is_credit(text: &str) -> bool {
    CREDIT_PREFIXES.iter().any(|p| {
        text.strip_prefix(p)
            .map(|r| r.trim_start().starts_with([':', '：']))
            .unwrap_or(false)
    })
}

/// Removes word-level `<mm:ss.xx>` tags.
fn strip_word_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            Some(close) if parse_time(&rest[open + 1..open + close]).is_some() => {
                out.push_str(&rest[..open]);
                rest = &rest[open + close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses LRC text into timed lines sorted by start time. Empty-text lines are kept as breaks;
/// metadata tags and credit lines are dropped.
pub fn parse(src: &str) -> Vec<LrcLine> {
    let mut out = Vec::new();
    for raw in src.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(close) = inner.find(']') else { break };
            match parse_time(&inner[..close]) {
                Some(t) => times.push(t),
                None => break,
            }
            rest = &inner[close + 1..];
        }
        if times.is_empty() {
            continue;
        }
        let text = strip_word_tags(rest).trim().to_string();
        if is_credit(&text) {
            continue;
        }
        out.extend(times.into_iter().map(|start_ms| LrcLine { start_ms, text: text.clone() }));
    }
    out.sort_by_key(|l| l.start_ms);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(start_ms: u32, text: &str) -> LrcLine {
        LrcLine { start_ms, text: text.into() }
    }

    #[test]
    fn parses() {
        let src = "[ar:x]\n[ti:y]\n[00:05.50][00:01.00]啊\n[00:02.00]作词：某人\n[00:03.00]曲 : 某人\n\
                   [00:09.00]\n[01:02.345] 你好 世界 \n[00:07]<00:07.10>字<00:07.50>词\nno tag line\n";
        assert_eq!(
            parse(src),
            vec![l(1000, "啊"), l(5500, "啊"), l(7000, "字词"), l(9000, ""), l(62345, "你好 世界")]
        );
    }
}
