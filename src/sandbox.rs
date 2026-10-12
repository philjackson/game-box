//! Confining a runner with bubblewrap.
//!
//! wine is not a security boundary: `Z:` is `/`, the prefix's "My Documents"
//! links into `$HOME`, and Windows code can make Linux syscalls directly. So
//! the confinement has to come from the kernel. `bwrap` starts the runner in
//! fresh mount, pid and (optionally) network namespaces where the only host
//! paths that exist are:
//!
//! - the system itself, read-only: `/usr`, `/etc`, `/opt`, `/sys`;
//! - the runner's install directory, read-only;
//! - the game's own directories, read-write;
//! - the display, audio and GPU/controller devices a game needs;
//! - whatever extra paths the game's settings name.
//!
//! `$HOME` is replaced by a private one kept per game under the data dir, so a
//! game's own dotfiles and shader caches persist without it ever seeing yours.
//!
//! Two consequences of the pid namespace shape the rest of the module. First,
//! bwrap tears the namespace down the moment the main process exits, and
//! wine's wineserver outlives that process to flush the registry — so the
//! command runs under a tiny `sh` that waits for the wineserver before
//! letting go. Second, a wineserver inside the sandbox cannot be reached from
//! outside through `/tmp`, so a forced stop signals the namespace instead.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::library;
use crate::model::{Runner, RunnerKind, Sandbox};

/// Environment variables worth passing through from our own environment.
/// Everything else is dropped: a shell's environment routinely carries API
/// tokens and agent sockets that a game has no business reading.
const PASS_EXACT: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LANGUAGE", "TZ",
    "XDG_RUNTIME_DIR", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS", "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XCURSOR_THEME",
    "XCURSOR_SIZE", "DRI_PRIME",
];
const PASS_PREFIX: &[&str] = &[
    "LC_", "WINE", "DXVK_", "VKD3D_", "PROTON_", "MANGOHUD", "SDL_", "__GL_", "__NV_",
    "MESA_", "RADV_", "AMD_", "VK_", "LIBGL_",
];

/// Everything that decides what one sandboxed run can see.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Names the private `$HOME` and wineserver directory. The game's root
    /// directory, so an install and the game it becomes share both.
    pub root: PathBuf,
    /// Paths that must exist, bound read-write.
    pub required: Vec<PathBuf>,
    pub read_write: Vec<PathBuf>,
    pub read_only: Vec<PathBuf>,
    pub network: bool,
    /// How many of the paths above came from the game's settings.
    pub extras: usize,
    /// gamescope runs inside with us and brings its own Xwayland, so the host
    /// X socket can stay out.
    pub gamescope: bool,
    /// The WINEPREFIX whose wineserver the wrapper waits for.
    pub prefix: PathBuf,
    pub wineserver: Option<PathBuf>,
    pub cwd: PathBuf,
}

impl Plan {
    /// The paths a runner needs whatever it is running, plus the user's extras.
    pub fn new(root: &Path, prefix: &Path, runner: &Runner, cfg: &Sandbox, cwd: &Path) -> Plan {
        let mut read_only: Vec<PathBuf> = vec![runner_root(runner)];
        read_only.extend(cfg.read_only.iter().cloned());
        if runner.kind == RunnerKind::Proton {
            // proton copies steam's legacy client dlls into the prefix when it
            // can find them. Only that folder: the rest of the Steam dir holds
            // login tokens.
            read_only.push(steam_root().join("legacycompat"));
        }
        Plan {
            root: root.to_path_buf(),
            required: vec![root.to_path_buf()],
            read_write: cfg.read_write.clone(),
            read_only,
            network: cfg.network,
            extras: cfg.read_only.len() + cfg.read_write.len(),
            gamescope: false,
            prefix: prefix.to_path_buf(),
            wineserver: crate::runners::wineserver_for(runner),
            cwd: cwd.to_path_buf(),
        }
    }

    /// The `bwrap` argv that runs `program args` inside, with `env` as the
    /// only environment on top of the passthrough list. Pure: nothing on disk
    /// changes, so the preview can call it every frame. [`Plan::prepare`]
    /// makes the directories it names.
    pub fn wrap(&self, program: &Path, args: &[String], env: &[(String, String)]) -> Result<Vec<String>> {
        let home = home();
        let mut out: Vec<String> = Vec::new();
        let mut push = |xs: &[&str]| out.extend(xs.iter().map(|s| s.to_string()));

        push(&["--unshare-pid", "--unshare-uts", "--unshare-cgroup-try"]);
        if !self.network {
            push(&["--unshare-net"]);
        }

        // --- the system, read-only -----------------------------------------
        push(&["--ro-bind", "/usr", "/usr"]);
        for top in ["bin", "sbin", "lib", "lib64", "lib32"] {
            let host = Path::new("/").join(top);
            match fs::read_link(&host) {
                // Merged-/usr distros make these symlinks; recreate them.
                Ok(target) => push(&["--symlink", &target.to_string_lossy(), &host.to_string_lossy()]),
                Err(_) if host.is_dir() => push(&["--ro-bind", &s(&host), &s(&host)]),
                Err(_) => {}
            }
        }
        push(&["--ro-bind", "/etc", "/etc", "--ro-bind-try", "/opt", "/opt"]);
        push(&["--ro-bind", "/sys", "/sys", "--ro-bind-try", "/run/udev", "/run/udev"]);
        if self.network {
            // /etc/resolv.conf is often a link into here.
            push(&["--ro-bind-try", "/run/systemd/resolve", "/run/systemd/resolve"]);
        }
        push(&["--proc", "/proc", "--dev", "/dev"]);

        // --- devices: GPU and controllers ----------------------------------
        push(&["--dev-bind-try", "/dev/dri", "/dev/dri", "--dev-bind-try", "/dev/input", "/dev/input"]);
        for dev in dev_nodes(&["nvidia", "hidraw"]) {
            push(&["--dev-bind", &s(&dev), &s(&dev)]);
        }

        // --- scratch, a private home, and the wineserver's socket dir -------
        push(&["--tmpfs", "/tmp"]);
        push(&["--bind", &s(&self.home_dir()), &s(&home)]);
        push(&["--bind", &s(&self.wine_dir()), &format!("/tmp/.wine-{}", crate::display::uid())]);

        // --- display and audio ----------------------------------------------
        let runtime = runtime_dir();
        push(&["--perms", "0700", "--dir", &s(&runtime)]);
        // A display recovered from the session's sockets arrives in `env`, not
        // our own environment, and its socket needs binding all the same.
        let wayland = env.iter().find(|(k, _)| k == "WAYLAND_DISPLAY").map(|(_, v)| v.clone())
            .or_else(|| std::env::var("WAYLAND_DISPLAY").ok())
            .filter(|v| !v.trim().is_empty());
        if let Some(w) = &wayland {
            // An absolute WAYLAND_DISPLAY is used as-is; join keeps it so.
            let sock = runtime.join(w);
            push(&["--ro-bind-try", &s(&sock), &s(&sock)]);
        }
        // With gamescope on a wayland desktop, gamescope's own Xwayland is the
        // only X server the game gets to talk to.
        let host_x = !(self.gamescope && wayland.is_some());
        if self.gamescope {
            push(&["--dir", "/tmp/.X11-unix"]);
        }
        if host_x {
            let display = env.iter().find(|(k, _)| k == "DISPLAY").map(|(_, v)| v.clone())
                .or_else(|| std::env::var("DISPLAY").ok());
            if let Some(n) = display.as_deref().and_then(x11_display_number) {
                let sock = format!("/tmp/.X11-unix/X{}", n);
                push(&["--ro-bind-try", &sock, &sock]);
            }
            let xauth = std::env::var("XAUTHORITY").ok().filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".Xauthority"));
            push(&["--ro-bind-try", &s(&xauth), &s(&xauth)]);
        }
        for sock in ["pipewire-0", "pulse/native"] {
            let p = runtime.join(sock);
            push(&["--ro-bind-try", &s(&p), &s(&p)]);
        }

        // --- the game, the runner, the extras -------------------------------
        // Parents before children, so a writable folder inside a read-only one
        // (or the reverse) ends up the way it was asked for.
        let mut binds: Vec<(&PathBuf, &str)> = Vec::new();
        binds.extend(self.read_only.iter().map(|p| (p, "--ro-bind-try")));
        binds.extend(self.read_write.iter().map(|p| (p, "--bind-try")));
        binds.extend(self.required.iter().map(|p| (p, "--bind")));
        binds.sort_by_key(|(p, _)| p.components().count());
        let mut seen: Vec<&PathBuf> = Vec::new();
        for (path, flag) in binds {
            check_exposure(path, &home)?;
            if seen.contains(&path) {
                continue;
            }
            seen.push(path);
            push(&[flag, &s(path), &s(path)]);
        }
        push(&["--chdir", &s(&self.cwd)]);

        // --- environment ----------------------------------------------------
        let mut vars: BTreeMap<String, String> = std::env::vars()
            .filter(|(k, _)| PASS_EXACT.contains(&k.as_str()) || PASS_PREFIX.iter().any(|p| k.starts_with(p)))
            .collect();
        vars.insert("HOME".into(), s(&home));
        vars.insert("XDG_RUNTIME_DIR".into(), s(&runtime));
        if !host_x {
            vars.remove("DISPLAY");
            vars.remove("XAUTHORITY");
        }
        for (k, v) in env {
            vars.insert(k.clone(), v.clone());
        }
        push(&["--clearenv"]);
        for (k, v) in &vars {
            push(&["--setenv", k, v]);
        }

        // --- the command, under a wrapper that outlasts the wineserver ------
        push(&["--", "/bin/sh", "-c", WRAPPER, "gbox-sandbox"]);
        push(&[
            &self.wineserver.as_deref().map(s).unwrap_or_default(),
            &s(&self.prefix),
            &s(program),
        ]);
        out.extend(args.iter().cloned());
        Ok(out)
    }

    /// Make the directories `wrap` binds. The private home also gets the
    /// folders the prefix's user-shell links point at — wine links "My
    /// Documents" and friends to `$HOME/Documents` and so on, and a game
    /// saving through a dangling link would fail.
    pub fn prepare(&self) -> Result<()> {
        let home_dir = self.home_dir();
        private_dir(&home_dir)?;
        private_dir(&self.wine_dir())?;
        let host_home = home();
        let users = self.prefix.join("drive_c/users");
        for user in fs::read_dir(&users).into_iter().flatten().flatten() {
            for entry in fs::read_dir(user.path()).into_iter().flatten().flatten() {
                let Ok(target) = fs::read_link(entry.path()) else { continue };
                if let Ok(rel) = target.strip_prefix(&host_home) {
                    let _ = fs::create_dir_all(home_dir.join(rel));
                }
            }
        }
        Ok(())
    }

    /// The private `$HOME`, persistent across runs.
    pub fn home_dir(&self) -> PathBuf {
        library::data_dir().join("sandbox").join(key(&self.root)).join("home")
    }

    /// Stands in for `/tmp/.wine-$UID`. Per game, so one sandboxed game
    /// cannot reach another's wineserver and ask it to start things.
    pub fn wine_dir(&self) -> PathBuf {
        runtime_dir().join("game-box").join(key(&self.root))
    }

    /// A short description for the detail pane, where the full bwrap line
    /// would bury everything else.
    pub fn describe(&self) -> String {
        format!(
            "bwrap [{}{}]",
            if self.network { "network" } else { "offline" },
            if self.extras > 0 { format!(", +{} paths", self.extras) } else { String::new() }
        )
    }
}

/// Runs `"$@"`, then waits for the prefix's wineserver so the namespace is
/// not torn down under a registry flush. A trapped (not ignored) TERM keeps
/// the wrapper alive through a polite stop without the game inheriting the
/// disposition.
const WRAPPER: &str = r#"ws=$1; pfx=$2; shift 2
trap : TERM INT
"$@"; s=$?
[ -n "$ws" ] && WINEPREFIX="$pfx" "$ws" -w
exit $s"#;

pub fn bwrap() -> Result<PathBuf> {
    crate::runners::which("bwrap")
        .ok_or_else(|| anyhow!("the sandbox is on but bwrap (bubblewrap) is not on $PATH"))
}

/// Binding `$HOME`, one of its parents or `/` would put everything back.
fn check_exposure(path: &Path, home: &Path) -> Result<()> {
    if path == Path::new("/") || home.starts_with(path) {
        return Err(anyhow!(
            "the sandbox would expose {} — that includes your whole home",
            path.display()
        ));
    }
    Ok(())
}

/// The directory a runner is installed in: proton's own folder, or the wine
/// tree above `bin/wine`.
fn runner_root(runner: &Runner) -> PathBuf {
    let up = match runner.kind {
        RunnerKind::Proton => runner.bin.parent(),
        RunnerKind::Wine => runner.bin.parent().and_then(Path::parent),
    };
    up.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/usr"))
}

pub fn steam_root() -> PathBuf {
    home().join(".local/share/Steam")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", crate::display::uid())))
}

/// `/dev` entries whose names start with any of `prefixes`.
fn dev_nodes(prefixes: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            prefixes.iter().any(|p| name.starts_with(p))
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// `:0`, `:1.0`, `unix:2` → the socket number. A TCP display has none.
fn x11_display_number(display: &str) -> Option<u32> {
    let rest = display.strip_prefix("unix").unwrap_or(display).strip_prefix(':')?;
    rest.split('.').next()?.parse().ok()
}

fn private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| anyhow!("cannot create {}: {}", dir.display(), e))
}

/// `iron-nest-1a2b3c4d`: readable, and unique per path.
fn key(root: &Path) -> String {
    let name = crate::model::slugify(&root.file_name().unwrap_or_default().to_string_lossy());
    // FNV-1a: stable across Rust releases, unlike the std hasher.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in root.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{}-{:08x}", name, h as u32)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> Runner {
        Runner {
            kind: RunnerKind::Wine,
            name: "w".into(),
            bin: PathBuf::from("/opt/wine-x/bin/wine"),
            origin: "t".into(),
        }
    }

    fn plan(cfg: &Sandbox) -> Plan {
        Plan::new(Path::new("/g/iron-nest"), Path::new("/g/iron-nest"), &runner(), cfg, Path::new("/g/iron-nest"))
    }

    #[test]
    fn keys_are_stable_readable_and_distinct() {
        assert_eq!(key(Path::new("/g/Iron Nest")), key(Path::new("/g/Iron Nest")));
        assert!(key(Path::new("/g/Iron Nest")).starts_with("iron-nest-"));
        assert_ne!(key(Path::new("/a/x")), key(Path::new("/b/x")));
    }

    #[test]
    fn x11_displays_parse_to_socket_numbers() {
        assert_eq!(x11_display_number(":0"), Some(0));
        assert_eq!(x11_display_number(":1.0"), Some(1));
        assert_eq!(x11_display_number("unix:2"), Some(2));
        assert_eq!(x11_display_number("host:0"), None);
    }

    #[test]
    fn the_game_is_writable_and_the_runner_is_not() {
        let argv = plan(&Sandbox::on()).wrap(Path::new("/opt/wine-x/bin/wine"), &["x.exe".into()], &[]).unwrap();
        let joined = argv.join(" ");
        assert!(joined.contains("--bind /g/iron-nest /g/iron-nest"));
        assert!(joined.contains("--ro-bind-try /opt/wine-x /opt/wine-x"));
        assert!(joined.contains("--unshare-pid"));
        assert!(!joined.contains("--unshare-net"), "network is on by default");
        assert!(joined.contains("--clearenv"));
        assert_eq!(argv.last().map(String::as_str), Some("x.exe"));
    }

    #[test]
    fn offline_unshares_the_network() {
        let cfg = Sandbox { network: false, ..Sandbox::on() };
        let argv = plan(&cfg).wrap(Path::new("/bin/true"), &[], &[]).unwrap();
        assert!(argv.iter().any(|a| a == "--unshare-net"));
    }

    #[test]
    fn exposing_home_or_root_is_refused() {
        for bad in [PathBuf::from("/"), home(), home().parent().unwrap().to_path_buf()] {
            let cfg = Sandbox { read_only: vec![bad.clone()], ..Sandbox::on() };
            assert!(plan(&cfg).wrap(Path::new("/bin/true"), &[], &[]).is_err(), "{}", bad.display());
        }
        let fine = Sandbox { read_write: vec![home().join("Saves")], ..Sandbox::on() };
        assert!(plan(&fine).wrap(Path::new("/bin/true"), &[], &[]).is_ok());
    }

    #[test]
    fn a_nested_writable_folder_mounts_after_its_read_only_parent() {
        let cfg = Sandbox {
            read_only: vec![PathBuf::from("/data/mods")],
            read_write: vec![PathBuf::from("/data/mods/saves")],
            ..Sandbox::on()
        };
        let argv = plan(&cfg).wrap(Path::new("/bin/true"), &[], &[]).unwrap();
        let at = |p: &str| argv.iter().position(|a| a == p).unwrap();
        assert!(at("/data/mods") < at("/data/mods/saves"));
    }

    #[test]
    fn a_recovered_wayland_display_has_its_socket_bound() {
        let argv = plan(&Sandbox::on())
            .wrap(Path::new("/bin/true"), &[], &[("WAYLAND_DISPLAY".into(), "wayland-9".into())])
            .unwrap();
        let sock = s(&runtime_dir().join("wayland-9"));
        assert!(argv.windows(3).any(|w| w == ["--ro-bind-try", sock.as_str(), sock.as_str()]));
    }

    #[test]
    fn secrets_in_our_environment_stay_out() {
        std::env::set_var("GBOX_TEST_TOKEN", "hunter2");
        let argv = plan(&Sandbox::on())
            .wrap(Path::new("/bin/true"), &[], &[("WINEDEBUG".into(), "-all".into())])
            .unwrap();
        std::env::remove_var("GBOX_TEST_TOKEN");
        assert!(!argv.iter().any(|a| a == "GBOX_TEST_TOKEN"));
        assert!(argv.windows(3).any(|w| w == ["--setenv", "WINEDEBUG", "-all"]));
    }
}
