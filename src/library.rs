//! Persistence: XDG paths and the JSON library file.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Game;

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("game-box")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn library_path() -> PathBuf {
    data_dir().join("library.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub games: Vec<Game>,
}

fn one() -> u32 {
    1
}

impl Library {
    pub fn load() -> Result<Library> {
        let path = library_path();
        if !path.exists() {
            return Ok(Library { version: 1, games: Vec::new() });
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let lib: Library = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(lib)
    }

    /// Atomic-ish save: write a sibling temp file and rename over the target.
    pub fn save(&self) -> Result<()> {
        let path = library_path();
        fs::create_dir_all(data_dir())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn unique_id(&self, base: &str) -> String {
        if !self.games.iter().any(|g| g.id == base) {
            return base.to_string();
        }
        for n in 2u32.. {
            let candidate = format!("{}-{}", base, n);
            if !self.games.iter().any(|g| g.id == candidate) {
                return candidate;
            }
        }
        unreachable!()
    }
}
