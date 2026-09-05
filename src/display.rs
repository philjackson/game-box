//! Finding the graphical session.
//!
//! wine needs `DISPLAY` or `WAYLAND_DISPLAY` to load a graphics driver. When
//! it has neither it prints
//!
//! ```text
//! err:winediag:nodrv_CreateWindow Application tried to create a window, but no driver could be loaded.
//! ```
//!
//! and the game sits there resident but blocked, drawing nothing — which looks
//! exactly like a game that launched fine. A terminal started outside the
//! graphical session (a bare TTY, a tmux server that outlived a login, a
//! systemd unit) has this gap, so we detect it and repair it from the sockets
//! the session leaves behind rather than launching into the void.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Display {
    /// Our own environment already names a display; nothing to do.
    Inherited { how: String },
    /// Nothing in the environment, but the session's sockets are on disk.
    Recovered { vars: Vec<(String, String)>, how: String },
    /// No display anywhere. Launching wine would produce an invisible game.
    Headless,
}

impl Display {
    /// Variables to add to a wine command's environment.
    pub fn vars(&self) -> Vec<(String, String)> {
        match self {
            Display::Recovered { vars, .. } => vars.clone(),
            _ => Vec::new(),
        }
    }

    pub fn is_headless(&self) -> bool {
        matches!(self, Display::Headless)
    }
}

fn env_display(key: &str) -> Option<String> {
    // An empty value is as useless as an absent one, and wine treats it the
    // same way, so do not let `DISPLAY=` count as having a display.
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

pub fn detect() -> Display {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", uid())));
    resolve(
        env_display("WAYLAND_DISPLAY"),
        env_display("DISPLAY"),
        &runtime,
        Path::new("/tmp/.X11-unix"),
    )
}

/// The decision itself, with everything it depends on passed in so the
/// headless case can be tested without taking a machine's display away.
pub fn resolve(
    env_wayland: Option<String>,
    env_x11: Option<String>,
    runtime: &Path,
    x11_dir: &Path,
) -> Display {
    match (&env_wayland, &env_x11) {
        (Some(w), Some(x)) => return Display::Inherited { how: format!("{} + {}", w, x) },
        (Some(w), None) => return Display::Inherited { how: w.clone() },
        (None, Some(x)) => return Display::Inherited { how: x.clone() },
        (None, None) => {}
    }

    let mut vars: Vec<(String, String)> = Vec::new();
    let mut found: Vec<String> = Vec::new();

    if let Some(sock) = first_wayland_socket(runtime) {
        vars.push(("WAYLAND_DISPLAY".into(), sock.clone()));
        found.push(sock);
    }
    if let Some(display) = first_x11_display(x11_dir) {
        vars.push(("DISPLAY".into(), display.clone()));
        found.push(display);
    }

    if vars.is_empty() {
        Display::Headless
    } else {
        Display::Recovered { how: found.join(" + "), vars }
    }
}

/// `wayland-0`, `wayland-1`, … in the runtime dir, lowest first.
pub fn first_wayland_socket(runtime: &Path) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(runtime)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let suffix = name.strip_prefix("wayland-")?;
            // `wayland-1.lock` is a lock file, not a socket.
            if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            Some(name)
        })
        .collect();
    names.sort_by_key(|n| n.strip_prefix("wayland-").and_then(|s| s.parse::<u32>().ok()));
    names.into_iter().next()
}

/// `X0`, `X1`, … in /tmp/.X11-unix, returned as `:0`, `:1`. Names like `X0_`
/// are Xwayland bookkeeping, not display sockets.
pub fn first_x11_display(dir: &Path) -> Option<String> {
    let mut nums: Vec<u32> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_prefix('X')?.parse::<u32>().ok()
        })
        .collect();
    nums.sort_unstable();
    nums.first().map(|n| format!(":{}", n))
}

fn uid() -> u32 {
    // Avoid a libc dependency for one number.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().next().map(str::to_string))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_sockets_sort_numerically_and_ignore_locks() {
        let dir = std::env::temp_dir().join("gbox-display-test-wl");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["wayland-10", "wayland-2", "wayland-2.lock", "wayland-", "other"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(first_wayland_socket(&dir), Some("wayland-2".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn x11_sockets_become_display_numbers_and_skip_xwayland_marks() {
        let dir = std::env::temp_dir().join("gbox-display-test-x11");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["X1", "X0_", "Xfoo"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(first_x11_display(&dir), Some(":1".into()));
        std::fs::write(dir.join("X0"), b"").unwrap();
        assert_eq!(first_x11_display(&dir), Some(":0".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_inherited_display_is_left_alone() {
        let d = resolve(
            Some("wayland-1".into()),
            Some(":0".into()),
            Path::new("/nonexistent"),
            Path::new("/nonexistent"),
        );
        assert!(matches!(d, Display::Inherited { .. }));
        assert!(d.vars().is_empty(), "must not override a working session");
    }

    #[test]
    fn a_shell_without_a_display_recovers_the_sessions_sockets() {
        let rt = std::env::temp_dir().join("gbox-display-test-recover-rt");
        let x11 = std::env::temp_dir().join("gbox-display-test-recover-x11");
        for d in [&rt, &x11] {
            let _ = std::fs::remove_dir_all(d);
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(rt.join("wayland-1"), b"").unwrap();
        std::fs::write(x11.join("X0"), b"").unwrap();

        let d = resolve(None, None, &rt, &x11);
        assert_eq!(
            d.vars(),
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
                ("DISPLAY".to_string(), ":0".to_string()),
            ]
        );
        assert!(!d.is_headless());
        for d in [&rt, &x11] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn no_env_and_no_sockets_is_headless() {
        let empty = std::env::temp_dir().join("gbox-display-test-headless");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        let d = resolve(None, None, &empty, &empty);
        assert!(d.is_headless(), "a launch here could only be invisible");
        assert!(d.vars().is_empty());
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn an_empty_display_variable_does_not_count() {
        // `DISPLAY=` is as useless to wine as no DISPLAY at all.
        let empty = std::env::temp_dir().join("gbox-display-test-blank");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        // env_display() filters blanks before resolve() ever sees them.
        assert_eq!(
            resolve(Some("  ".into()), None, &empty, &empty),
            Display::Inherited { how: "  ".into() },
            "resolve trusts its caller; the filtering lives in env_display"
        );
        std::env::set_var("GBOX_TEST_BLANK", "   ");
        assert_eq!(env_display("GBOX_TEST_BLANK"), None);
        std::env::remove_var("GBOX_TEST_BLANK");
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn nothing_on_disk_means_headless() {
        let empty = std::env::temp_dir().join("gbox-display-test-empty");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(first_wayland_socket(&empty), None);
        assert_eq!(first_x11_display(&empty), None);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
