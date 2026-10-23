//! CC-CEDICT download, parsing and lookup.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use karaoke_core::model::DictEntry;
use karaoke_core::pinyin::numbered_to_marked_multi;

const URL: &str = "https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz";
const FILE_NAME: &str = "cedict_1_0_ts_utf-8_mdbg.txt.gz";

/// Returns the cached CEDICT archive path, downloading it first if missing.
pub fn ensure_downloaded(client: &reqwest::blocking::Client, cache_dir: &Path) -> Result<PathBuf> {
    let path = cache_dir.join(FILE_NAME);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(cache_dir).with_context(|| format!("creating {}", cache_dir.display()))?;
    println!("cedict: downloading {URL}");
    let resp = client.get(URL).timeout(Duration::from_secs(300)).send()?;
    if !resp.status().is_success() {
        bail!("cedict: HTTP {} from {URL}", resp.status());
    }
    let bytes = resp.bytes()?;
    let tmp = path.with_extension("gz.part");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub dict: DictEntry,
    /// Raw CEDICT reading, lowercased (`yue4 liang5`, `lu:4`).
    pub numbered: String,
}

pub struct Cedict {
    map: HashMap<String, Vec<Entry>>,
}

impl Cedict {
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let (dict, skipped) = Self::parse(BufReader::new(GzDecoder::new(file)))?;
        if skipped > 0 {
            println!("cedict: skipped {skipped} malformed lines");
        }
        Ok(dict)
    }

    /// Parses CEDICT text; returns the dictionary and the number of malformed lines skipped.
    pub fn parse(reader: impl BufRead) -> Result<(Self, usize)> {
        let mut map: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut skipped = 0;
        for line in reader.lines() {
            let line = line?;
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((trad, simp, raw_reading, defs)) = parse_line(line) else {
                skipped += 1;
                continue;
            };
            let entry = Entry {
                dict: DictEntry {
                    traditional: trad.to_string(),
                    simplified: simp.to_string(),
                    reading: numbered_to_marked_multi(raw_reading),
                    defs,
                },
                numbered: raw_reading.to_lowercase(),
            };
            if trad != simp {
                map.entry(trad.to_string()).or_default().push(entry.clone());
            }
            map.entry(simp.to_string()).or_default().push(entry);
        }
        // Common-word readings before proper nouns (capitalised pinyin); stable otherwise.
        for entries in map.values_mut() {
            entries.sort_by_key(|e| e.dict.reading.chars().next().is_some_and(char::is_uppercase));
        }
        Ok((Self { map }, skipped))
    }

    pub fn get(&self, key: &str) -> &[Entry] {
        self.map.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn dict_entries(&self, key: &str) -> Vec<DictEntry> {
        self.get(key).iter().map(|e| e.dict.clone()).collect()
    }
}

/// `TRAD SIMP [pin1 yin1] /d1/d2/`
fn parse_line(line: &str) -> Option<(&str, &str, &str, Vec<String>)> {
    let (trad, rest) = line.split_once(' ')?;
    let (simp, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('[')?;
    let (reading, rest) = rest.split_once(']')?;
    let body = rest.trim_start().strip_prefix('/')?.strip_suffix('/')?;
    let defs: Vec<String> = body.split('/').filter(|d| !d.is_empty()).map(str::to_string).collect();
    if trad.is_empty() || simp.is_empty() || defs.is_empty() {
        return None;
    }
    Some((trad, simp, reading, defs))
}

#[cfg(test)]
pub fn fixture() -> Cedict {
    let src = "# CC-CEDICT\n的 的 [de5] /possessive particle/\n月亮 月亮 [yue4 liang5] /the moon/\n\
               愛 爱 [ai4] /to love/\n亮 亮 [liang4] /bright/\nbroken line\n";
    Cedict::parse(src.as_bytes()).unwrap().0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_both_forms() {
        let (d, skipped) = Cedict::parse(
            "# c\n的 的 [de5] /possessive particle/\n月亮 月亮 [yue4 liang5] /the moon/\n愛 爱 [ai4] /to love/\nbad\n"
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(skipped, 1);
        assert_eq!(d.get("的").len(), 1);
        assert_eq!(d.get("的")[0].dict.reading, "de");
        assert_eq!(d.get("月亮")[0].dict.reading, "yuè liang");
        assert_eq!(d.get("月亮")[0].numbered, "yue4 liang5");
        assert_eq!(d.get("月亮")[0].dict.defs, vec!["the moon"]);
        assert_eq!(d.get("爱")[0].dict.reading, "ài");
        assert_eq!(d.get("愛")[0].dict.simplified, "爱");
        assert!(d.get("月").is_empty());
    }

    #[test]
    fn proper_nouns_last() {
        let (d, _) =
            Cedict::parse("鄧 邓 [Deng4] /surname Deng/\n鄧 邓 [deng4] /x/\n".as_bytes()).unwrap();
        let r: Vec<&str> = d.get("邓").iter().map(|e| e.numbered.as_str()).collect();
        assert_eq!(r, vec!["deng4", "deng4"]);
        assert_eq!(d.get("邓")[1].dict.reading, "Dèng");
    }
}
