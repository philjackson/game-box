//! Inspecting a directory on disk: is it a wine prefix, and what can we launch?

use std::path::{Component, Path, PathBuf};

use walkdir::WalkDir;

/// A launchable executable found under a scanned directory.
#[derive(Debug, Clone)]
pub struct ExeCandidate {
    pub path: PathBuf,
    /// Path relative to the scan root, for display.
    pub rel: String,
    pub size: u64,
    /// Higher is more likely to be the game. See [`score_exe`].
    pub score: i32,
}

/// What we learned about a directory the user pointed at.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub root: PathBuf,
    /// Detected WINEPREFIX, if the directory is (or contains) one.
    pub prefix: Option<PathBuf>,
    /// Proton build recorded in the prefix's `version` file.
    pub runner_hint: Option<String>,
    pub candidates: Vec<ExeCandidate>,
    pub truncated: bool,
}

/// Filename fragments that are never the game itself.
const EXE_DENY: &[&str] = &[
    "unins", "uninstall", "crashhandler", "crashreport", "crashpad", "vcredist",
    "dxsetup", "directx", "dotnet", "setup", "installer", "redist", "launcher_installer",
    "eossetup", "epicwebhelper", "ue4prereq", "ueprereq", "oalinst", "notepad",
    "regedit", "winecfg", "iexplore", "wmplayer", "explorer", "conhost", "python",
    "quicksfv", "xnafx", "physx", "openal", "-kb", "prereq", "activation",
];

/// Any path component with one of these names is skipped wholesale. Matching
/// per component (rather than on the path prefix) catches the redist folder
/// wherever a publisher chose to bury it.
const DIR_DENY_COMPONENT: &[&str] = &[
    "windows", "programdata", "appdata", "openxr", "vrclient", "dosdevices",
    "shadercache", "glcache", "d3d12", "gstreamer-1.0", "common files",
    "internet explorer", "windows media player", "windows nt", "windows mail",
    "_redist", "redist", "_commonredist", "commonredist", "directx", "vcredist",
    "dotnet", "dotnetfx", "prerequisites", "prereq", "_support", "extras",
    "thirdparty", "installers", "drivers",
];

/// Is this directory the root of a wine prefix?
pub fn is_prefix(dir: &Path) -> bool {
    dir.join("drive_c").is_dir() && (dir.join("system.reg").is_file() || dir.join("user.reg").is_file())
}

/// Find the prefix associated with `dir`: the directory itself, a `pfx`
/// subdirectory (proton compatdata layout), or nothing.
pub fn find_prefix(dir: &Path) -> Option<PathBuf> {
    if is_prefix(dir) {
        return Some(dir.to_path_buf());
    }
    let pfx = dir.join("pfx");
    if is_prefix(&pfx) {
        return Some(pfx);
    }
    // The user may have pointed at a game folder living *inside* a prefix.
    let mut cur = dir;
    for _ in 0..6 {
        let parent = cur.parent()?;
        if is_prefix(parent) {
            return Some(parent.to_path_buf());
        }
        cur = parent;
    }
    None
}

/// Strip everything but letters and digits, so "S.T.A.L.K.E.R. 2",
/// "stalker-2" and "Stalker2" all compare equal.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Score an executable by how likely it is to be the game's entry point.
fn score_exe(root: &Path, path: &Path, size: u64, depth: usize) -> i32 {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut score = 0;

    // Size is weak evidence: a 66 MB installer beats a 248 KB Unreal launcher
    // on bytes alone, so keep the spread small.
    score += match size {
        0..=100_000 => -25,
        100_001..=1_000_000 => 0,
        1_000_001..=20_000_000 => 12,
        _ => 18,
    };

    // Shallow beats deep.
    score -= (depth as i32) * 6;

    // A name matching the directory it sits in — or the directory the user
    // pointed us at — is the strongest signal we have.
    let root_squash = squash(&root.file_name().unwrap_or_default().to_string_lossy());
    let name_squash = squash(&name);
    if !root_squash.is_empty() {
        if name_squash == root_squash {
            score += 40;
        } else if name_squash.starts_with(&root_squash) || root_squash.starts_with(&name_squash) {
            score += 25;
        }
    }
    if let Some(parent) = path.parent().and_then(|p| p.file_name()) {
        if squash(&parent.to_string_lossy()) == name_squash {
            score += 20;
        }
    }

    // Unity ships `<Game>_Data` next to `<Game>.exe`.
    if let Some(dir) = path.parent() {
        if dir.join(format!("{}_Data", path.file_stem().unwrap_or_default().to_string_lossy())).is_dir() {
            score += 35;
        }
    }

    // Shipping builds of Unreal titles live deep but always work.
    if name.ends_with("-win64-shipping") || name.ends_with("-win32-shipping") {
        score += 45;
    }

    score
}

fn denied(name: &str) -> bool {
    EXE_DENY.iter().any(|d| name.contains(d))
}

fn denied_dir(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    // The prefix's own windows install is the one path we match from the root,
    // so a game legitimately called "Windows ..." elsewhere still scans.
    if lower == "drive_c/windows" || lower.starts_with("drive_c/windows/") {
        return true;
    }
    lower
        .rsplit('/')
        .next()
        .map(|last| DIR_DENY_COMPONENT.contains(&last))
        .unwrap_or(false)
}

/// Walk `root` looking for candidate executables. Bounded so pointing at a
/// 200 GB prefix does not stall the UI.
pub fn scan_dir(root: &Path, max_depth: usize, limit: usize) -> ScanResult {
    let prefix = find_prefix(root);
    let runner_hint = prefix
        .as_ref()
        .and_then(|p| {
            let compat = if p.file_name().map(|n| n == "pfx").unwrap_or(false) {
                p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.clone())
            } else {
                p.clone()
            };
            crate::runners::prefix_runner_hint(&compat)
        });

    let mut candidates: Vec<ExeCandidate> = Vec::new();
    let mut truncated = false;
    let mut seen = 0usize;

    let walker = WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if !e.file_type().is_dir() {
                return true;
            }
            let Ok(rel) = e.path().strip_prefix(root) else { return true };
            if rel.as_os_str().is_empty() {
                return true;
            }
            !denied_dir(&rel.to_string_lossy())
        });

    for entry in walker.flatten() {
        seen += 1;
        if seen > 400_000 {
            truncated = true;
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let is_exe = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("exe"))
            .unwrap_or(false);
        if !is_exe {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if denied(&stem) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let depth = entry.depth().saturating_sub(1);
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        candidates.push(ExeCandidate {
            score: score_exe(root, path, size, depth),
            path: path.to_path_buf(),
            rel,
            size,
        });
    }

    candidates.sort_by(|a, b| b.score.cmp(&a.score).then(b.size.cmp(&a.size)));
    if candidates.len() > limit {
        candidates.truncate(limit);
        truncated = true;
    }

    ScanResult { root: root.to_path_buf(), prefix, runner_hint, candidates, truncated }
}

/// Recursive apparent size of a directory, capped by a file count budget so a
/// huge tree degrades to an estimate instead of hanging.
pub fn dir_size(root: &Path, budget: usize) -> u64 {
    let mut total = 0u64;
    for (n, entry) in WalkDir::new(root).follow_links(false).into_iter().flatten().enumerate() {
        if n > budget {
            break;
        }
        if let Ok(md) = entry.metadata() {
            if md.is_file() {
                total += md.len();
            }
        }
    }
    total
}

/// Directory listing used by the path prompts' completion. With `with_exes`
/// it also offers Windows executables — what the installer prompt needs.
pub fn complete_entries(input: &str, with_exes: bool) -> Vec<PathBuf> {
    let expanded = expand_tilde(input);
    let (dir, needle) = if input.ends_with('/') {
        (expanded.clone(), String::new())
    } else {
        (
            expanded.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/")),
            expanded
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() || (with_exes && is_windows_exe(p)))
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            needle.is_empty() || name.to_ascii_lowercase().starts_with(&needle.to_ascii_lowercase())
        })
        .collect();
    // Directories first, then executables, each alphabetically.
    out.sort_by_key(|p| (!p.is_dir(), p.to_string_lossy().to_lowercase()));
    out
}

pub fn is_windows_exe(p: &Path) -> bool {
    p.is_file()
        && p.extension()
            .map(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("msi"))
            .unwrap_or(false)
}

/// A path the user typed, ready to be stored or handed to a child process:
/// `~` expanded and the whole thing made absolute.
pub fn user_path(input: &str) -> PathBuf {
    absolute(&expand_tilde(input))
}

/// Make a path absolute without requiring it to exist, folding away `.` and
/// `..` rather than resolving symlinks.
///
/// A relative path cannot survive the trip into a runner: we hand wine and
/// proton both a working directory and a prefix, and they resolve the second
/// against the first — so `Games/x` becomes `Games/x/Games/x` and the launch
/// fails somewhere far away from the prompt that accepted it.
pub fn absolute(path: &Path) -> PathBuf {
    // Nothing typed is still nothing — not the working directory.
    if path.as_os_str().is_empty() {
        return PathBuf::new();
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")).join(rest)
    } else if input == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    } else {
        PathBuf::from(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn squash_normalises_punctuation() {
        assert_eq!(squash("S.T.A.L.K.E.R. 2"), "stalker2");
        assert_eq!(squash("stalker-2"), "stalker2");
        assert_eq!(squash("Stalker2"), "stalker2");
    }

    #[test]
    fn redist_directories_are_skipped_at_any_depth() {
        assert!(denied_dir("drive_c/Games/Foo/_Redist"));
        assert!(denied_dir("Engine/Extras"));
        assert!(denied_dir("drive_c/windows"));
        assert!(denied_dir("drive_c/windows/system32"));
        assert!(!denied_dir("drive_c/Games/Windows Central"));
        assert!(!denied_dir("drive_c/Games/Prodeus"));
    }

    /// A scratch directory that cleans itself up.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let dir = std::env::temp_dir().join(format!("gbox-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The check that decides whether the add wizard offers to build a prefix.
    #[test]
    fn a_bare_game_directory_is_not_mistaken_for_a_prefix() {
        let tmp = Tmp::new("bare");
        let game = tmp.0.join("MyGame");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("MyGame.exe"), b"MZ").unwrap();

        // Game files but no wine infrastructure: this is what we offer to fix.
        assert!(!is_prefix(&game));
        assert_eq!(find_prefix(&game), None);

        // drive_c alone is not enough — a stray folder of that name is common.
        fs::create_dir_all(game.join("drive_c")).unwrap();
        assert!(!is_prefix(&game));

        // wineboot leaves the registry behind, and that settles it.
        fs::write(game.join("system.reg"), b"WINE REGISTRY").unwrap();
        assert!(is_prefix(&game));
        assert_eq!(find_prefix(&game), Some(game.clone()));
    }

    /// Proton builds into `<dir>/pfx`, which is where we look next.
    #[test]
    fn a_proton_prefix_is_found_through_its_pfx_subdirectory() {
        let tmp = Tmp::new("pfx");
        let game = tmp.0.join("MyGame");
        let pfx = game.join("pfx");
        fs::create_dir_all(pfx.join("drive_c")).unwrap();
        fs::write(pfx.join("user.reg"), b"WINE REGISTRY").unwrap();

        assert!(!is_prefix(&game));
        assert_eq!(find_prefix(&game), Some(pfx));
    }

    /// The bug this guards: a relative prefix is resolved a second time by
    /// the runner, against the working directory we just gave it.
    #[test]
    fn user_paths_come_back_absolute() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(user_path("Games/x"), cwd.join("Games/x"));
        assert_eq!(user_path("./Games/./x"), cwd.join("Games/x"));
        assert_eq!(user_path("Games/y/../x"), cwd.join("Games/x"));
        assert_eq!(user_path("/opt/games/x"), PathBuf::from("/opt/games/x"));
        assert_eq!(user_path("~/Games/x"), dirs::home_dir().unwrap().join("Games/x"));
        // Absolute already, and no existence check anywhere.
        assert!(user_path("/nowhere/at/all").is_absolute());
        // `..` never climbs above the root.
        assert_eq!(user_path("/../../x"), PathBuf::from("/x"));
        // An empty prompt is not a request for the working directory.
        assert_eq!(user_path(""), PathBuf::new());
    }

    #[test]
    fn installers_never_outrank_a_matching_game_name() {
        let root = Path::new("/games/stalker-2");
        let game = score_exe(
            root,
            Path::new("/games/stalker-2/drive_c/Games/S.T.A.L.K.E.R. 2/Stalker2.exe"),
            248_000,
            3,
        );
        let redist = score_exe(
            root,
            Path::new("/games/stalker-2/drive_c/Games/S.T.A.L.K.E.R. 2/_Redist/NDP471.exe"),
            66_000_000,
            4,
        );
        assert!(game > redist, "game {} should beat redist {}", game, redist);
    }
}
