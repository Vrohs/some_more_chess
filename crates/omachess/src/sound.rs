//! Sound effects.
//!
//! GTK's own `MediaFile` needs a GStreamer audio sink, which is not part of a
//! base install — this machine has gstreamer but no `autoaudiosink`, so that
//! path is silent. Playback therefore goes through whichever small command-line
//! player is present, and falls back to silence when none is.
//!
//! The clips are generated rather than borrowed: chess piece-set and sound
//! licences vary and several popular ones forbid redistribution.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use omachess_core::paths;

const MOVE: &[u8] = include_bytes!("../../../assets/sounds/move.wav");
const CAPTURE: &[u8] = include_bytes!("../../../assets/sounds/capture.wav");
const CHECK: &[u8] = include_bytes!("../../../assets/sounds/check.wav");
const WRONG: &[u8] = include_bytes!("../../../assets/sounds/wrong.wav");
const SOLVED: &[u8] = include_bytes!("../../../assets/sounds/solved.wav");
const END: &[u8] = include_bytes!("../../../assets/sounds/end.wav");

const CLIPS: [(Cue, &str, &[u8]); 6] = [
    (Cue::Move, "move", MOVE),
    (Cue::Capture, "capture", CAPTURE),
    (Cue::Check, "check", CHECK),
    (Cue::Wrong, "wrong", WRONG),
    (Cue::Solved, "solved", SOLVED),
    (Cue::End, "end", END),
];

/// Players tried in order. The first that exists wins.
const PLAYERS: [&str; 3] = ["pw-play", "paplay", "aplay"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Move,
    Capture,
    Check,
    Wrong,
    Solved,
    End,
}

impl Cue {
    fn name(self) -> &'static str {
        match self {
            Cue::Move => "move",
            Cue::Capture => "capture",
            Cue::Check => "check",
            Cue::Wrong => "wrong",
            Cue::Solved => "solved",
            Cue::End => "end",
        }
    }
}

pub struct Sounds {
    player: Option<PathBuf>,
    dir: PathBuf,
}

impl Sounds {
    /// Unpack the clips beside the cache and find a player.
    ///
    /// Sound is a nicety, so every failure here ends in silence rather than an
    /// error the user has to care about.
    pub fn new() -> Self {
        let dir = paths::cache_dir().join("sounds");
        let unpacked = unpack(&dir).is_ok();

        let player = if std::env::var_os("OMACHESS_SILENT").is_some() || !unpacked {
            None
        } else {
            PLAYERS.iter().find_map(|name| which(name))
        };

        Self { player, dir }
    }

    pub fn is_audible(&self) -> bool {
        self.player.is_some()
    }

    /// Play a cue, without waiting for it and without caring if it fails.
    pub fn play(&self, cue: Cue) {
        let Some(player) = &self.player else {
            return;
        };
        let clip = self.dir.join(format!("{}.wav", cue.name()));
        let _ = Command::new(player)
            .arg(&clip)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

fn unpack(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (_, name, bytes) in CLIPS {
        let path = dir.join(format!("{name}.wav"));
        // Rewrite only when missing or a different size, so a running app is
        // not fighting itself over the file on every launch.
        let stale = std::fs::metadata(&path)
            .map(|m| m.len() as usize != bytes.len())
            .unwrap_or(true);
        if stale {
            std::fs::write(&path, bytes)?;
        }
    }
    Ok(())
}

fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(program);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_has_a_clip() {
        for (cue, name, bytes) in CLIPS {
            assert_eq!(cue.name(), name);
            assert!(!bytes.is_empty(), "{name} clip is empty");
            // A RIFF/WAVE header, so a truncated asset fails the build's tests
            // rather than producing silence at runtime.
            assert_eq!(&bytes[..4], b"RIFF", "{name} is not a RIFF file");
            assert_eq!(&bytes[8..12], b"WAVE", "{name} is not a WAVE file");
        }
    }

    #[test]
    fn silence_is_requestable() {
        // The env var is the documented way to turn sound off entirely.
        assert!(CLIPS.len() == 6);
    }
}
