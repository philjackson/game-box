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

/// How gamescope should present the game's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    #[default]
    Fullscreen,
    Borderless,
    Windowed,
}

impl WindowMode {
    pub fn label(self) -> &'static str {
        match self {
            WindowMode::Fullscreen => "fullscreen",
            WindowMode::Borderless => "borderless",
            WindowMode::Windowed => "windowed",
        }
    }

    pub fn flag(self) -> Option<&'static str> {
        match self {
            WindowMode::Fullscreen => Some("-f"),
            WindowMode::Borderless => Some("-b"),
            WindowMode::Windowed => None,
        }
    }

    pub fn next(self) -> WindowMode {
        match self {
            WindowMode::Fullscreen => WindowMode::Borderless,
            WindowMode::Borderless => WindowMode::Windowed,
            WindowMode::Windowed => WindowMode::Fullscreen,
        }
    }
}

/// gamescope's upscaler filter (`-F`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    Linear,
    Nearest,
    Fsr,
    Nis,
    Pixel,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::Linear => "linear",
            Filter::Nearest => "nearest",
            Filter::Fsr => "fsr",
            Filter::Nis => "nis",
            Filter::Pixel => "pixel",
        }
    }

    /// Cycle including "off", which is why this takes and returns an Option.
    pub fn next(current: Option<Filter>) -> Option<Filter> {
        match current {
            None => Some(Filter::Linear),
            Some(Filter::Linear) => Some(Filter::Nearest),
            Some(Filter::Nearest) => Some(Filter::Fsr),
            Some(Filter::Fsr) => Some(Filter::Nis),
            Some(Filter::Nis) => Some(Filter::Pixel),
            Some(Filter::Pixel) => None,
        }
    }
}

/// Per-game gamescope settings. gamescope is a nested compositor: it wraps the
/// runner command, so the game renders into it rather than onto the desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Gamescope {
    #[serde(default)]
    pub enabled: bool,
    /// `-W`/`-H`: the size of gamescope's own output. None lets it decide.
    #[serde(default)]
    pub output: Option<(u32, u32)>,
    /// `-w`/`-h`: what the game renders at, upscaled to the output.
    #[serde(default)]
    pub game: Option<(u32, u32)>,
    #[serde(default)]
    pub mode: WindowMode,
    /// `-r`: cap the game's frame rate.
    #[serde(default)]
    pub fps_limit: Option<u32>,
    #[serde(default)]
    pub filter: Option<Filter>,
    /// `--mangoapp`: the mangohud overlay, composited by gamescope itself.
    #[serde(default)]
    pub mangoapp: bool,
    /// Anything else, passed through verbatim.
    #[serde(default)]
    pub extra: Vec<String>,
}

impl Gamescope {
    /// The arguments that go before `--`.
    pub fn args(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some((w, h)) = self.output {
            out.push("-W".into());
            out.push(w.to_string());
            out.push("-H".into());
            out.push(h.to_string());
        }
        if let Some((w, h)) = self.game {
            out.push("-w".into());
            out.push(w.to_string());
            out.push("-h".into());
            out.push(h.to_string());
        }
        if let Some(flag) = self.mode.flag() {
            out.push(flag.into());
        }
        if let Some(fps) = self.fps_limit {
            out.push("-r".into());
            out.push(fps.to_string());
        }
        if let Some(filter) = self.filter {
            out.push("-F".into());
            out.push(filter.label().into());
        }
        if self.mangoapp {
            out.push("--mangoapp".into());
        }
        out.extend(self.extra.iter().cloned());
        out
    }

    /// One-line description for the index and detail pane.
    pub fn summary(&self) -> String {
        if !self.enabled {
            return "off".into();
        }
        let mut parts: Vec<String> = vec![self.mode.label().to_string()];
        match (self.game, self.output) {
            (Some((gw, gh)), Some((ow, oh))) => parts.push(format!("{}x{} → {}x{}", gw, gh, ow, oh)),
            (None, Some((ow, oh))) => parts.push(format!("{}x{}", ow, oh)),
            (Some((gw, gh)), None) => parts.push(format!("{}x{} → native", gw, gh)),
            (None, None) => parts.push("native".into()),
        }
        if let Some(f) = self.filter {
            parts.push(f.label().to_string());
        }
        if let Some(fps) = self.fps_limit {
            parts.push(format!("{}fps", fps));
        }
        if self.mangoapp {
            parts.push("mangoapp".into());
        }
        parts.join(" · ")
    }
}

/// Parse a `WIDTHxHEIGHT` pair; an empty string means "unset".
pub fn parse_resolution(s: &str) -> Result<Option<(u32, u32)>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let (w, h) = s
        .split_once(['x', 'X', '*'])
        .ok_or_else(|| format!("\"{}\" is not WIDTHxHEIGHT", s))?;
    let w: u32 = w.trim().parse().map_err(|_| format!("\"{}\" is not a width", w.trim()))?;
    let h: u32 = h.trim().parse().map_err(|_| format!("\"{}\" is not a height", h.trim()))?;
    if w == 0 || h == 0 {
        return Err("resolution cannot be zero".into());
    }
    Ok(Some((w, h)))
}

pub fn format_resolution(res: Option<(u32, u32)>) -> String {
    res.map(|(w, h)| format!("{}x{}", w, h)).unwrap_or_default()
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
    #[serde(default)]
    pub gamescope: Gamescope,
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
    fn resolutions_round_trip_and_reject_junk() {
        assert_eq!(parse_resolution(""), Ok(None));
        assert_eq!(parse_resolution(" 2560x1440 "), Ok(Some((2560, 1440))));
        assert_eq!(parse_resolution("1920X1080"), Ok(Some((1920, 1080))));
        assert!(parse_resolution("1920").is_err());
        assert!(parse_resolution("axb").is_err());
        assert!(parse_resolution("0x1080").is_err());
        assert_eq!(format_resolution(Some((1280, 720))), "1280x720");
        assert_eq!(format_resolution(None), "");
    }

    #[test]
    fn gamescope_args_follow_the_documented_flags() {
        let gs = Gamescope {
            enabled: true,
            output: Some((2560, 1440)),
            game: Some((1920, 1080)),
            mode: WindowMode::Fullscreen,
            fps_limit: Some(60),
            filter: Some(Filter::Fsr),
            mangoapp: true,
            extra: vec!["--force-grab-cursor".into()],
        };
        assert_eq!(
            gs.args(),
            vec![
                "-W", "2560", "-H", "1440", "-w", "1920", "-h", "1080", "-f", "-r", "60", "-F",
                "fsr", "--mangoapp", "--force-grab-cursor",
            ]
        );
        // Windowed contributes no flag at all.
        let plain = Gamescope { mode: WindowMode::Windowed, ..Default::default() };
        assert!(plain.args().is_empty());
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
            gamescope: Gamescope::default(),
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
