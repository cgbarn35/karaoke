use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

pub enum Playback {
    Audio {
        _sink: rodio::MixerDeviceSink,
        player: rodio::Player,
    },
    Sync {
        base_ms: f64,
        started: Option<Instant>,
    },
}

impl Playback {
    pub fn sync() -> Self {
        Playback::Sync {
            base_ms: 0.0,
            started: None,
        }
    }

    /// Opens the default output device and queues `path`, paused at 0.
    pub fn audio(path: &Path) -> anyhow::Result<Self> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink()?;
        let player = rodio::Player::connect_new(sink.mixer());
        player.append(rodio::Decoder::try_from(File::open(path)?)?);
        player.pause();
        Ok(Playback::Audio {
            _sink: sink,
            player,
        })
    }

    pub fn is_audio(&self) -> bool {
        matches!(self, Playback::Audio { .. })
    }

    pub fn now_ms(&self) -> i64 {
        match self {
            Playback::Audio { player, .. } => player.get_pos().as_millis() as i64,
            Playback::Sync { base_ms, started } => {
                let elapsed = started.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0);
                (base_ms + elapsed) as i64
            }
        }
    }

    pub fn play(&mut self) {
        match self {
            Playback::Audio { player, .. } => player.play(),
            Playback::Sync { started, .. } => {
                if started.is_none() {
                    *started = Some(Instant::now());
                }
            }
        }
    }

    pub fn pause(&mut self) {
        match self {
            Playback::Audio { player, .. } => player.pause(),
            Playback::Sync { base_ms, started } => {
                if let Some(s) = started.take() {
                    *base_ms += s.elapsed().as_secs_f64() * 1000.0;
                }
            }
        }
    }

    /// Seeks to `ms` (clamped at 0) on the playback clock.
    pub fn seek(&mut self, ms: i64) -> Result<(), String> {
        let ms = ms.max(0);
        match self {
            Playback::Audio { player, .. } => player
                .try_seek(Duration::from_millis(ms as u64))
                .map_err(|e| format!("Seek failed: {e}")),
            Playback::Sync { base_ms, started } => {
                *base_ms = ms as f64;
                if started.is_some() {
                    *started = Some(Instant::now());
                }
                Ok(())
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        match self {
            Playback::Audio { player, .. } => !player.is_paused() && !player.empty(),
            Playback::Sync { started, .. } => started.is_some(),
        }
    }

    pub fn finished(&self, duration_ms: u32) -> bool {
        match self {
            Playback::Audio { player, .. } => player.empty(),
            Playback::Sync { .. } => self.now_ms() >= i64::from(duration_ms),
        }
    }
}
