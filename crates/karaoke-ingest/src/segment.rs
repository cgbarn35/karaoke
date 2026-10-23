//! Line tokenisation: jieba words, split into Han / non-Han runs.

use jieba_rs::Jieba;
use karaoke_core::is_han;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub text: String,
    pub is_han: bool,
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Han,
    Alnum,
    Other,
}

fn class(c: char) -> Class {
    if is_han(c) {
        Class::Han
    } else if c.is_alphanumeric() {
        Class::Alnum
    } else {
        Class::Other
    }
}

/// Segments a lyric line. Mixed pieces are split into Han and non-Han runs;
/// every space or punctuation char becomes its own token.
pub fn tokenize_line(jieba: &Jieba, text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for piece in jieba.cut(text, true) {
        let mut cur = String::new();
        let mut cur_class = None;
        for c in piece.word.chars() {
            let k = class(c);
            if cur_class != Some(k) || k == Class::Other {
                if !cur.is_empty() {
                    out.push(Token { text: std::mem::take(&mut cur), is_han: cur_class == Some(Class::Han) });
                }
                cur_class = Some(k);
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            out.push(Token { text: cur, is_han: cur_class == Some(Class::Han) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_runs() {
        let jieba = Jieba::new();
        let toks = tokenize_line(&jieba, "你好 世界, OK吗");
        let joined: String = toks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(joined, "你好 世界, OK吗");
        for t in &toks {
            assert_eq!(t.is_han, t.text.chars().all(is_han), "{t:?}");
            assert!(t.is_han || t.text.chars().all(char::is_alphanumeric) || t.text.chars().count() == 1, "{t:?}");
        }
        assert!(toks.contains(&Token { text: " ".into(), is_han: false }));
        assert!(toks.contains(&Token { text: ",".into(), is_han: false }));
        assert!(toks.contains(&Token { text: "OK".into(), is_han: false }));
    }
}
