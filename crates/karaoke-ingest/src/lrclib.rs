//! LRCLIB (lrclib.net) synced-lyrics lookup.

use anyhow::{Result, bail};
use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: u64,
    pub album_name: Option<String>,
    pub duration: Option<f64>,
    #[serde(default)]
    pub instrumental: bool,
    pub synced_lyrics: Option<String>,
}

impl Record {
    fn has_synced(&self) -> bool {
        !self.instrumental && self.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
}

fn check(resp: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    if !resp.status().is_success() {
        bail!("lrclib: HTTP {}: {}", resp.status(), resp.text().unwrap_or_default());
    }
    Ok(resp)
}

pub fn get(client: &Client, id: u64) -> Result<Record> {
    let rec: Record = check(client.get(format!("https://lrclib.net/api/get/{id}")).send()?)?.json()?;
    if !rec.has_synced() {
        bail!("LRCLIB record {id} has no synced lyrics");
    }
    Ok(rec)
}

pub fn search(client: &Client, title: &str, artist: &str, duration_s: Option<f64>) -> Result<Record> {
    let recs: Vec<Record> = check(
        client
            .get("https://lrclib.net/api/search")
            .query(&[("track_name", title), ("artist_name", artist)])
            .send()?,
    )?
    .json()?;
    let mut usable = recs.into_iter().filter(Record::has_synced);
    let picked = match duration_s {
        Some(d) => usable.min_by(|a, b| {
            let da = (a.duration.unwrap_or(f64::INFINITY) - d).abs();
            let db = (b.duration.unwrap_or(f64::INFINITY) - d).abs();
            da.total_cmp(&db)
        }),
        None => usable.next(),
    };
    match picked {
        Some(r) => Ok(r),
        None => bail!("no synced lyrics on LRCLIB for \"{title}\" by \"{artist}\"; pass --lrc <file>"),
    }
}
