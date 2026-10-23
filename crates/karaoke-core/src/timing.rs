//! Line end times and evenly interpolated per-character times.

use crate::is_han;
use crate::lrc::LrcLine;
use crate::model::Line;

/// End time for each LRC line. Lyric lines end 300 ms before the next line (at least 500 ms long);
/// break lines run to the next line; the last line lasts up to 8 s, capped by the song duration.
pub fn line_ends(lines: &[LrcLine], duration_ms: u32) -> Vec<u32> {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| match lines.get(i + 1) {
            None => (l.start_ms + 8000).min(duration_ms).max(l.start_ms + 500),
            Some(next) if l.text.is_empty() => next.start_ms,
            Some(next) => (l.start_ms + 500).max(next.start_ms.saturating_sub(300)),
        })
        .collect()
}

fn char_is_han(s: &str) -> bool {
    s.chars().next().is_some_and(is_han)
}

/// Splits `[start_ms, end_ms)` evenly across the line's Han chars (remainder to the last one).
/// Non-Han chars become zero-length at the start of the next Han char, or at `end_ms`.
pub fn assign_char_times(line: &mut Line) {
    let n = line.words.iter().flat_map(|w| &w.chars).filter(|c| char_is_han(&c.ch)).count() as u32;
    let span = line.end_ms.saturating_sub(line.start_ms);
    let step = span.checked_div(n).unwrap_or(0);
    let mut k = 0u32;
    for c in line.words.iter_mut().flat_map(|w| &mut w.chars) {
        if char_is_han(&c.ch) {
            c.start_ms = line.start_ms + k * step;
            k += 1;
            c.end_ms = if k == n { line.end_ms } else { line.start_ms + k * step };
        }
    }
    // Non-Han chars: zero-length at the next Han start (walk backwards).
    let mut next = line.end_ms;
    for c in line.words.iter_mut().rev().flat_map(|w| w.chars.iter_mut().rev()) {
        if char_is_han(&c.ch) {
            next = c.start_ms;
        } else {
            c.start_ms = next;
            c.end_ms = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Char, Word};

    fn word(text: &str) -> Word {
        let is_han = text.chars().all(is_han);
        Word {
            text: text.into(),
            is_han,
            reading: String::new(),
            gloss: String::new(),
            dict: vec![],
            chars: text
                .chars()
                .map(|c| Char {
                    ch: c.to_string(),
                    reading: String::new(),
                    gloss: String::new(),
                    dict: vec![],
                    start_ms: 0,
                    end_ms: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn chars_split_evenly() {
        let mut line = Line {
            start_ms: 1000,
            end_ms: 2000,
            text: "你好 世界!".into(),
            translation: String::new(),
            words: vec![word("你好"), word(" "), word("世界"), word("!")],
        };
        assign_char_times(&mut line);
        let times: Vec<(u32, u32)> =
            line.words.iter().flat_map(|w| &w.chars).map(|c| (c.start_ms, c.end_ms)).collect();
        assert_eq!(
            times,
            vec![(1000, 1250), (1250, 1500), (1500, 1500), (1500, 1750), (1750, 2000), (2000, 2000)]
        );
    }

    #[test]
    fn remainder_goes_to_last() {
        let mut line = Line {
            start_ms: 0,
            end_ms: 1000,
            text: "一二三".into(),
            translation: String::new(),
            words: vec![word("一二三")],
        };
        assign_char_times(&mut line);
        let c = &line.words[0].chars;
        assert_eq!((c[0].start_ms, c[1].start_ms, c[2].start_ms, c[2].end_ms), (0, 333, 666, 1000));
    }

    #[test]
    fn ends() {
        let l = |s: u32, t: &str| LrcLine { start_ms: s, text: t.into() };
        let lines = [l(1000, "a"), l(5000, ""), l(6000, "b"), l(6200, "c"), l(9000, "d")];
        assert_eq!(line_ends(&lines, 12_000), vec![4700, 6000, 6500, 8700, 12_000]);
        assert_eq!(line_ends(&[l(1000, "a")], 20_000), vec![9000]);
        assert_eq!(line_ends(&[l(1000, "a")], 1200), vec![1500]);
    }
}
