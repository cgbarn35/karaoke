//! Pinyin tone-number ↔ tone-mark conversion.

const MARKS: [(char, [char; 4]); 6] = [
    ('a', ['ā', 'á', 'ǎ', 'à']),
    ('e', ['ē', 'é', 'ě', 'è']),
    ('i', ['ī', 'í', 'ǐ', 'ì']),
    ('o', ['ō', 'ó', 'ǒ', 'ò']),
    ('u', ['ū', 'ú', 'ǔ', 'ù']),
    ('ü', ['ǖ', 'ǘ', 'ǚ', 'ǜ']),
];
/// Combining marks for syllabic m/n (e.g. `ng2` → `ńg`).
const COMBINING: [char; 4] = ['\u{304}', '\u{301}', '\u{30C}', '\u{300}'];

fn mark(base: char, tone: usize) -> Option<char> {
    let lower = base.to_lowercase().next()?;
    let (_, tones) = MARKS.iter().find(|(b, _)| *b == lower)?;
    let m = tones[tone - 1];
    Some(if base.is_uppercase() { m.to_uppercase().next().unwrap_or(m) } else { m })
}

/// Converts one numbered syllable (`yue4`, `lu:4`, `nv3`, `Deng4`) to tone marks.
/// Tone 5 or a missing digit adds no mark; case is preserved.
pub fn numbered_to_marked(syl: &str) -> String {
    let (body, tone) = match syl.chars().last().and_then(|c| c.to_digit(10)) {
        Some(d @ 1..=5) => (&syl[..syl.len() - 1], Some(d as usize)),
        _ => (syl, None),
    };
    let mut s = body.replace("u:", "ü").replace("U:", "Ü");
    if tone.is_some() {
        s = s.replace('v', "ü").replace('V', "Ü");
    }
    let Some(tone @ 1..=4) = tone else { return s };
    let chars: Vec<char> = s.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let pos = lower
        .iter()
        .position(|&c| c == 'a')
        .or_else(|| lower.iter().position(|&c| c == 'e'))
        .or_else(|| lower.windows(2).position(|w| w == ['o', 'u']))
        .or_else(|| lower.iter().rposition(|c| matches!(c, 'i' | 'o' | 'u' | 'ü')));
    match pos {
        Some(p) => {
            let mut out: String = chars[..p].iter().collect();
            out.push(mark(chars[p], tone).unwrap_or(chars[p]));
            out.extend(&chars[p + 1..]);
            out
        }
        None => match lower.iter().position(|c| matches!(c, 'm' | 'n')) {
            Some(p) => {
                let mut out: String = chars[..=p].iter().collect();
                out.push(COMBINING[tone - 1]);
                out.extend(&chars[p + 1..]);
                out
            }
            None => s,
        },
    }
}

/// Converts space-separated numbered syllables (`yue4 liang5` → `yuè liang`).
pub fn numbered_to_marked_multi(s: &str) -> String {
    s.split_whitespace().map(numbered_to_marked).collect::<Vec<_>>().join(" ")
}

/// Removes tone marks (ü → u) and lowercases, for tone-insensitive search.
pub fn strip_tones(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !('\u{300}'..='\u{36F}').contains(c))
        .map(|c| {
            if c == 'ü' {
                return 'u';
            }
            MARKS
                .iter()
                .find(|(_, tones)| tones.contains(&c))
                .map(|(b, _)| if *b == 'ü' { 'u' } else { *b })
                .unwrap_or(c)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks() {
        for (i, o) in [
            ("yue4", "yuè"),
            ("liang5", "liang"),
            ("lu:4", "lǜ"),
            ("nv3", "nǚ"),
            ("gou3", "gǒu"),
            ("xiu1", "xiū"),
            ("gui4", "guì"),
            ("er2", "ér"),
            ("Deng4", "Dèng"),
            ("Ai4", "Ài"),
            ("ng2", "n\u{301}g"),
        ] {
            assert_eq!(numbered_to_marked(i), o, "{i}");
        }
        assert_eq!(numbered_to_marked_multi("yue4 liang5"), "yuè liang");
    }

    #[test]
    fn strip() {
        assert_eq!(strip_tones("Yuèliang"), "yueliang");
        assert_eq!(strip_tones("lǜ NǙ"), "lu nu");
    }
}
