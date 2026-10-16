//! Persistence: XDG paths and the JSON library file.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Game, Gamescope};

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
    #[serde(default)]
    pub defaults: Defaults,
}

/// What a newly added game starts with. Each game keeps its own copy, so
/// changing these later leaves games already in the library alone.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub gamescope: Gamescope,
}

fn one() -> u32 {
    1
}

impl Library {
    pub fn load() -> Result<Library> {
        let path = library_path();
        if !path.exists() {
            return Ok(Library { version: 1, ..Library::default() });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_saved_before_defaults_existed_still_loads() {
        let lib: Library = serde_json::from_str(r#"{"version": 1, "games": []}"#).unwrap();
        assert!(!lib.defaults.gamescope.enabled);
        assert_eq!(lib.defaults.gamescope.output, None);
    }

    #[test]
    fn defaults_survive_a_round_trip() {
        let mut lib = Library { version: 1, ..Library::default() };
        lib.defaults.gamescope.enabled = true;
        lib.defaults.gamescope.output = Some((3440, 1440));
        let back: Library = serde_json::from_str(&serde_json::to_string(&lib).unwrap()).unwrap();
        assert_eq!(back.defaults.gamescope, lib.defaults.gamescope);
    }
}
