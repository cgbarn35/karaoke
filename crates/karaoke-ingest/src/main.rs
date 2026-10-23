use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

mod cedict;
mod llm;
mod lrclib;
mod pipeline;
mod segment;

#[derive(Parser)]
#[command(version, about = "Ingest songs into the karaoke SQLite database")]
struct Cli {
    /// Database path.
    #[arg(long, default_value = "dist/karaoke.db")]
    db: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fetch lyrics, analyse them with CC-CEDICT + an OpenRouter model, and store the song.
    Add(AddArgs),
    /// Copy a user-owned audio file next to the DB and attach it to a song.
    AttachAudio {
        #[arg(long)]
        song_id: i64,
        #[arg(long)]
        audio: PathBuf,
    },
    /// List stored songs.
    List,
}

#[derive(Args)]
pub struct AddArgs {
    #[arg(long)]
    pub title: String,
    #[arg(long)]
    pub artist: String,
    /// Use this LRCLIB record instead of searching.
    #[arg(long, conflicts_with = "lrc")]
    pub lrclib_id: Option<u64>,
    /// Use a local .lrc file instead of LRCLIB.
    #[arg(long)]
    pub lrc: Option<PathBuf>,
    /// Song duration in seconds (picks the closest LRCLIB match; required length for --lrc).
    #[arg(long)]
    pub duration_s: Option<f64>,
    /// Audio file to copy next to the DB.
    #[arg(long)]
    pub audio: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Add(args) => pipeline::add(&cli.db, &args),
        Cmd::AttachAudio { song_id, audio } => pipeline::attach_audio(&cli.db, song_id, &audio),
        Cmd::List => pipeline::list(&cli.db),
    }
}
