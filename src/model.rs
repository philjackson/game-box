//! Core data model: games, runners, and the on-disk library.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How a game gets executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnerKind {
    /// A Proton distribution (GE-Proton, UMU-Proton, valve Proton...).
    Proton,
    /// A plain wine tree (system wine, lutris wine-ge, wine-staging...).
    Wine,
}

impl RunnerKind {
    pub fn label(self) -> &'static str {
        match self {
            RunnerKind::Proton => "proton",
            RunnerKind::Wine => "wine",
        }
    }
}

/// A runner installation discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runner {
    pub kind: RunnerKind,
    /// Display name, e.g. `GE-Proton11-6`.
    pub name: String,
    /// For proton: the `proton` script. For wine: the `wine` binary.
    pub bin: PathBuf,
    /// Where it came from, shown in the picker.
    pub origin: String,
}

/// The runner a game is pinned to. Resolved against the discovered list at
/// launch time so the library survives a runner being upgraded or removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerRef {
    pub kind: RunnerKind,
    pub name: String,
}

impl RunnerRef {
    pub fn of(r: &Runner) -> Self {
        RunnerRef { kind: r.kind, name: r.name.clone() }
    }
}

/// One entry in the library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    /// Host-side path to the windows executable.
    pub exe: PathBuf,
    /// WINEPREFIX root (the directory holding `drive_c`).
    pub prefix: PathBuf,
    pub runner: RunnerRef,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    pub added: DateTime<Utc>,
    #[serde(default)]
    pub last_played: Option<DateTime<Utc>>,
    #[serde(default)]
    pub playtime_secs: u64,
    /// Minutes per session, most recent last. Feeds the activity sparkline.
    #[serde(default)]
    pub sessions: Vec<u32>,
    /// Cached prefix size in bytes; recomputed lazily in the background.
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

impl Game {
    /// Directory the exe lives in — the default working directory.
    pub fn exe_dir(&self) -> PathBuf {
        self.exe.parent().map(Path::to_path_buf).unwrap_or_else(|| self.prefix.clone())
    }

    pub fn cwd(&self) -> PathBuf {
        self.working_dir.clone().unwrap_or_else(|| self.exe_dir())
    }

    /// Proton wants STEAM_COMPAT_DATA_PATH: the directory *containing* `pfx`.
    pub fn compat_data_path(&self) -> PathBuf {
        if self.prefix.join("pfx").exists() {
            self.prefix.clone()
        } else if self.prefix.file_name().map(|n| n == "pfx").unwrap_or(false) {
            self.prefix.parent().map(Path::to_path_buf).unwrap_or_else(|| self.prefix.clone())
        } else {
            self.prefix.clone()
        }
    }

    /// Proton needs a real WINEPREFIX at `<compat>/pfx`; flat lutris-style
    /// prefixes fake it with a `pfx -> .` symlink.
    pub fn is_proton_ready(&self) -> bool {
        self.compat_data_path().join("pfx").join("drive_c").is_dir()
    }

    pub fn exists(&self) -> bool {
        self.exe.is_file()
    }
}

/// Make a filesystem- and url-safe id out of a display name.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("game");
    }
    out
}

/// Turn `iron-nest` or `Iron Nest Heavy Turret Simulator.exe` into a title.
pub fn titleize(s: &str) -> String {
    let cleaned = s.replace(['_', '-', '.'], " ");
    let mut out = String::new();
    for word in cleaned.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut cs = word.chars();
        match cs.next() {
            Some(c) => {
                out.extend(c.to_uppercase());
                out.push_str(cs.as_str());
            }
            None => {}
        }
    }
    out
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{}{}", bytes, UNITS[i])
    } else if v < 10.0 {
        format!("{:.1}{}", v, UNITS[i])
    } else {
        format!("{:.0}{}", v, UNITS[i])
    }
}

pub fn human_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h{:02}", h, m)
        }
    }
}

/// mutt-ish relative date for the index column.
pub fn relative_date(when: Option<DateTime<Utc>>) -> String {
    let Some(when) = when else { return "never".into() };
    let delta = Utc::now().signed_duration_since(when);
    let mins = delta.num_minutes();
    if mins < 1 {
        "now".into()
    } else if mins < 60 {
        format!("{}m ago", mins)
    } else if mins < 60 * 24 {
        format!("{}h ago", mins / 60)
    } else if mins < 60 * 24 * 30 {
        format!("{}d ago", mins / (60 * 24))
    } else {
        when.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_title_round_trip_sensibly() {
        assert_eq!(slugify("S.T.A.L.K.E.R. 2: Heart of Chornobyl"), "s-t-a-l-k-e-r-2-heart-of-chornobyl");
        assert_eq!(titleize("iron-nest"), "Iron Nest");
        assert_eq!(titleize("unlucky_mummy"), "Unlucky Mummy");
    }

    #[test]
    fn sizes_and_durations_are_compact() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(4 * 1024 * 1024 * 1024), "4.0G");
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(3600), "1h");
        assert_eq!(human_duration(3600 + 25 * 60), "1h25");
    }

    #[test]
    fn flat_lutris_prefix_resolves_its_own_compat_path() {
        let g = Game {
            id: "x".into(),
            name: "x".into(),
            exe: PathBuf::from("/g/x/x.exe"),
            prefix: PathBuf::from("/g/x"),
            runner: RunnerRef { kind: RunnerKind::Proton, name: "GE".into() },
            args: vec![],
            env: Default::default(),
            working_dir: None,
            tags: vec![],
            notes: String::new(),
            added: Utc::now(),
            last_played: None,
            playtime_secs: 0,
            sessions: vec![],
            size_bytes: None,
        };
        // No pfx/ on disk in the test env, so it falls back to the prefix root.
        assert_eq!(g.compat_data_path(), PathBuf::from("/g/x"));
        assert_eq!(g.exe_dir(), PathBuf::from("/g/x"));
    }
}
