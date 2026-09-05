//! Discovery of Proton and Wine installations.

use std::path::{Path, PathBuf};

use crate::model::{Runner, RunnerKind, RunnerRef};

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Directories that hold one subdirectory per Proton build.
fn proton_roots() -> Vec<(PathBuf, &'static str)> {
    let h = home();
    vec![
        (h.join(".local/share/Steam/compatibilitytools.d"), "steam"),
        (h.join(".steam/root/compatibilitytools.d"), "steam"),
        (h.join(".local/share/Steam/steamapps/common"), "steamapps"),
        (PathBuf::from("/usr/share/steam/compatibilitytools.d"), "system"),
    ]
}

/// Directories that hold one subdirectory per wine build.
fn wine_roots() -> Vec<(PathBuf, &'static str)> {
    let h = home();
    vec![
        (h.join(".local/share/lutris/runners/wine"), "lutris"),
        (h.join(".local/share/wine"), "local"),
        (PathBuf::from("/opt/wine"), "opt"),
    ]
}

/// Scan the filesystem for everything we know how to launch with.
/// Results are deduplicated by name and sorted newest-looking first.
pub fn discover() -> Vec<Runner> {
    let mut out: Vec<Runner> = Vec::new();

    for (root, origin) in proton_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            let script = dir.join("proton");
            if !script.is_file() {
                continue;
            }
            let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
            if out.iter().any(|r| r.name == name) {
                continue;
            }
            out.push(Runner { kind: RunnerKind::Proton, name, bin: script, origin: origin.to_string() });
        }
    }

    for (root, origin) in wine_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            let bin = ["bin/wine", "files/bin/wine", "usr/bin/wine"]
                .iter()
                .map(|p| dir.join(p))
                .find(|p| p.is_file());
            let Some(bin) = bin else { continue };
            let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
            if out.iter().any(|r| r.name == name) {
                continue;
            }
            out.push(Runner { kind: RunnerKind::Wine, name, bin, origin: origin.to_string() });
        }
    }

    if let Some(bin) = which("wine") {
        if !out.iter().any(|r| r.name == "system") {
            out.push(Runner {
                kind: RunnerKind::Wine,
                name: "system".into(),
                bin,
                origin: "PATH".into(),
            });
        }
    }

    // Proton first (it is what most of these prefixes were built with), then
    // reverse-alphabetical so GE-Proton11-6 lands above GE-Proton10-32.
    out.sort_by(|a, b| match (a.kind, b.kind) {
        (RunnerKind::Proton, RunnerKind::Wine) => std::cmp::Ordering::Less,
        (RunnerKind::Wine, RunnerKind::Proton) => std::cmp::Ordering::Greater,
        _ => natural_cmp(&b.name, &a.name),
    });
    out
}

pub fn resolve<'a>(runners: &'a [Runner], want: &RunnerRef) -> Option<&'a Runner> {
    runners
        .iter()
        .find(|r| r.kind == want.kind && r.name == want.name)
        // Fall back to any runner of the right kind so a renamed/upgraded
        // build still launches instead of hard-failing.
        .or_else(|| runners.iter().find(|r| r.kind == want.kind))
}

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(bin))
        .find(|p| p.is_file())
}

/// Compare names so that embedded numbers sort numerically ("10" < "11").
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let xn = take_number(&mut ai);
                let yn = take_number(&mut bi);
                match xn.cmp(&yn) {
                    std::cmp::Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(x), Some(y)) => {
                ai.next();
                bi.next();
                match x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()) {
                    std::cmp::Ordering::Equal => {}
                    other => return other,
                }
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u64 {
    let mut n = 0u64;
    while let Some(c) = it.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        n = n.saturating_mul(10).saturating_add(c as u64 - '0' as u64);
        it.next();
    }
    n
}

/// The `wineserver` binary belonging to a runner. Killing a stuck prefix
/// means talking to its wineserver: the processes it supervises detach from
/// our process group, so signalling the child alone leaves them behind.
pub fn wineserver_for(runner: &Runner) -> Option<PathBuf> {
    let candidates = match runner.kind {
        RunnerKind::Wine => vec![runner.bin.with_file_name("wineserver")],
        RunnerKind::Proton => {
            let root = runner.bin.parent()?;
            vec![
                root.join("files/bin/wineserver"),
                root.join("dist/bin/wineserver"),
            ]
        }
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// Ask a prefix's wineserver to shut everything in it down. Fire and forget:
/// the caller is a UI thread and must not block.
pub fn kill_prefix(wineserver: &Path, prefix: &Path) {
    let _ = std::process::Command::new(wineserver)
        .arg("-k")
        .env("WINEPREFIX", prefix)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Read the Proton build a lutris/proton prefix was last created with.
pub fn prefix_runner_hint(prefix: &Path) -> Option<String> {
    let version = prefix.join("version");
    let raw = std::fs::read_to_string(version).ok()?;
    let first = raw.lines().next()?.trim().to_string();
    if first.is_empty() {
        None
    } else {
        Some(first)
    }
}
