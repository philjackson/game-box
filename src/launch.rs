//! Building and spawning the wine/proton command line.

use std::fs::{self, File};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use anyhow::{anyhow, Result};
use chrono::Utc;

use crate::library;
use crate::display::{self, Display};
use crate::model::{Game, Runner, RunnerKind};
use crate::runners;
use crate::sandbox::{self, Plan};

/// A game we have started and are still watching.
#[derive(Debug)]
pub struct Session {
    pub game_id: String,
    pub child: Child,
    pub pid: u32,
    pub started: std::time::Instant,
    pub log: PathBuf,
    pub runner: String,
    /// The WINEPREFIX this session is using, for a forced shutdown.
    pub prefix: PathBuf,
    pub wineserver: Option<PathBuf>,
    /// Runs inside bwrap: the wineserver is out of reach, and the pid
    /// namespace is what a forced stop takes down.
    pub sandboxed: bool,
    /// Set once the user has asked for a graceful stop, so a second press can
    /// escalate.
    pub kill_requested: bool,
}

type Argv = (PathBuf, Vec<String>, Vec<(String, String)>);

/// The exact command that would be run, for the confirm screen and the log
/// header. Returned as (program, args, env); a sandboxed game's env travels
/// inside the bwrap arguments instead.
pub fn build_command(game: &Game, runner: &Runner) -> Result<Argv> {
    let (bin, args, env) = runner_command(game, runner)?;
    match sandbox_plan(game, runner) {
        Some(plan) => Ok((sandbox::bwrap()?, plan.wrap(&bin, &args, &env)?, Vec::new())),
        None => Ok((bin, args, env)),
    }
}

/// What a sandboxed game may see, or None when its sandbox is off.
pub fn sandbox_plan(game: &Game, runner: &Runner) -> Option<Plan> {
    if !game.sandbox.enabled {
        return None;
    }
    let prefix = match runner.kind {
        RunnerKind::Proton => game.compat_data_path().join("pfx"),
        RunnerKind::Wine => game.prefix.clone(),
    };
    let mut plan = Plan::new(&game.root_dir(), &prefix, runner, &game.sandbox, &game.cwd());
    // The game's files do not always sit beside its prefix.
    for dir in [game.prefix.clone(), game.compat_data_path(), game.exe_dir(), game.cwd()] {
        if !dir.as_os_str().is_empty() && !dir.starts_with(&plan.root) {
            plan.read_write.push(dir);
        }
    }
    // stdout and stderr are this file, and wine asks for their names at
    // startup: a path that does not exist inside makes proton's ntdll crash.
    plan.read_write.push(library::log_dir().join(format!("{}.log", game.id)));
    plan.gamescope = game.gamescope.enabled;
    Some(plan)
}

/// The runner invocation itself, before any sandbox goes around it.
fn runner_command(game: &Game, runner: &Runner) -> Result<Argv> {
    let exe = game
        .exe
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not valid UTF-8"))?
        .to_string();

    // Without a display wine loads no graphics driver and the game runs
    // resident but invisible, which reads as a successful launch.
    let display = display::detect();
    if display.is_headless() {
        return Err(anyhow!(
            "no graphical session: neither DISPLAY nor WAYLAND_DISPLAY is set and no \
             wayland or X socket was found. Run game-box from inside your desktop \
             session, or export the display first"
        ));
    }

    let mut env: Vec<(String, String)> = display.vars();
    let mut args: Vec<String> = Vec::new();

    match runner.kind {
        RunnerKind::Proton => {
            let compat = game.compat_data_path();
            if !compat.join("pfx").exists() {
                return Err(anyhow!(
                    "proton needs a prefix at {}/pfx (a flat prefix wants a `pfx -> .` symlink)",
                    compat.display()
                ));
            }
            let steam_root = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".local/share/Steam");
            env.push(("STEAM_COMPAT_DATA_PATH".into(), compat.to_string_lossy().into()));
            env.push((
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                steam_root.to_string_lossy().into(),
            ));
            // Steam sets this; without it the NVIDIA driver wipes its shader
            // cache once it outgrows the default size, and a big game
            // recompiles everything on the next launch.
            if std::env::var_os("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP").is_none() {
                env.push(("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP".into(), "1".into()));
            }
            args.push("run".into());
            args.push(exe);
        }
        RunnerKind::Wine => {
            env.push(("WINEPREFIX".into(), game.prefix.to_string_lossy().into()));
            // Keep err:/warn: so the log viewer can explain a failed launch.
            env.push(("WINEDEBUG".into(), "fixme-all".into()));
            args.push(exe);
        }
    }

    args.extend(game.args.iter().cloned());
    for (k, v) in &game.env {
        env.push((k.clone(), v.clone()));
    }

    // gamescope is a nested compositor: it takes its own options, then `--`,
    // then the command it should host — so it becomes the program we exec.
    if game.gamescope.enabled {
        let bin = runners::which("gamescope")
            .ok_or_else(|| anyhow!("gamescope is enabled for this game but is not on $PATH"))?;
        let mut wrapped = game.gamescope.args();
        wrapped.push("--".into());
        wrapped.push(runner.bin.to_string_lossy().into_owned());
        wrapped.extend(args);
        return Ok((bin, wrapped, env));
    }

    Ok((runner.bin.clone(), args, env))
}

/// Render the command for the detail pane. A sandboxed one is summarised:
/// the full bwrap line runs to dozens of arguments and would bury the rest.
pub fn command_preview(game: &Game, runner: &Runner) -> String {
    let built = build_command(game, runner).and_then(|_| runner_command(game, runner));
    match (built, sandbox_plan(game, runner)) {
        (Ok(argv), Some(plan)) => format!("{} {}", plan.describe(), shell_line(&argv)),
        (Ok(argv), None) => shell_line(&argv),
        (Err(e), _) => format!("(cannot launch: {})", e),
    }
}

/// The whole command, copy-pasteable, for the log header.
pub fn command_line(game: &Game, runner: &Runner) -> String {
    match build_command(game, runner) {
        Ok(argv) => shell_line(&argv),
        Err(e) => format!("(cannot launch: {})", e),
    }
}

pub fn shell_line((bin, args, env): &Argv) -> String {
    let mut s = String::new();
    for (k, v) in env {
        s.push_str(&format!("{}={} ", k, shell_quote(v)));
    }
    s.push_str(&shell_quote(&bin.to_string_lossy()));
    for a in args {
        s.push(' ');
        s.push_str(&shell_quote(a));
    }
    s
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// How the display was resolved, for the status line after a launch.
pub fn display_note() -> Option<String> {
    match display::detect() {
        Display::Recovered { how, .. } => Some(format!(
            "this shell has no DISPLAY — using the session's {}",
            how
        )),
        _ => None,
    }
}

/// Spawn the game detached from our stdio, logging to the data dir.
pub fn launch(game: &Game, runner: &Runner) -> Result<Session> {
    if !game.exe.is_file() {
        return Err(anyhow!("executable is missing: {}", game.exe.display()));
    }
    let (bin, args, env) = build_command(game, runner)?;
    let plan = sandbox_plan(game, runner);
    if let Some(plan) = &plan {
        plan.prepare()?;
    }

    fs::create_dir_all(library::log_dir())?;
    let log_path = library::log_dir().join(format!("{}.log", game.id));
    let mut log = File::create(&log_path)?;
    {
        use std::io::Write;
        writeln!(log, "# game-box session {}", Utc::now().to_rfc3339())?;
        writeln!(log, "# runner {} ({})", runner.name, runner.kind.label())?;
        writeln!(log, "# {}", command_line(game, runner))?;
        writeln!(log)?;
    }
    let err_log = log.try_clone()?;

    // Own process group, so we can signal the whole wine tree later and so
    // ^C in the TUI never reaches the game.
    let mut cmd = Command::new(&bin);
    cmd.args(&args)
        .envs(env)
        .current_dir(game.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err_log));
    detach(&mut cmd, plan.is_some());
    let child = cmd.spawn()?;

    let prefix = match runner.kind {
        RunnerKind::Proton => game.compat_data_path().join("pfx"),
        RunnerKind::Wine => game.prefix.clone(),
    };

    Ok(Session {
        game_id: game.id.clone(),
        pid: child.id(),
        child,
        started: std::time::Instant::now(),
        log: log_path,
        runner: runner.name.clone(),
        prefix,
        wineserver: crate::runners::wineserver_for(runner),
        sandboxed: plan.is_some(),
        kill_requested: false,
    })
}

/// Put a child in a process group of its own. A sandboxed one gets a whole
/// session instead, which leaves it without a controlling terminal: through
/// `/dev/tty` it could otherwise push keystrokes into the shell behind the
/// TUI. The session id doubles as the group id, so signalling is unchanged.
pub fn detach(cmd: &mut Command, sandboxed: bool) {
    if sandboxed {
        unsafe {
            cmd.pre_exec(|| {
                if libc_setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    } else {
        cmd.process_group(0);
    }
}

/// Ask a running session to quit. The first call is polite; a second one goes
/// through the prefix's wineserver, which is the only thing that reliably
/// reaches wine processes — they leave our process group behind.
pub fn terminate(session: &mut Session) -> &'static str {
    if !session.kill_requested {
        session.kill_requested = true;
        unsafe {
            libc_kill(-(session.pid as i32), 15);
        }
        let _ = session.child.kill();
        return "asked the game to quit — press x again to force the prefix down";
    }
    match &session.wineserver {
        // The sandbox's pid 1 is in our group; killing it takes every process
        // in the namespace — wineserver included — down with it.
        _ if session.sandboxed => {
            unsafe {
                libc_kill(-(session.pid as i32), 9);
            }
            "killing everything in the sandbox"
        }
        Some(ws) => {
            crate::runners::kill_prefix(ws, &session.prefix);
            "killing every process in the prefix"
        }
        None => {
            unsafe {
                libc_kill(-(session.pid as i32), 9);
            }
            "sent SIGKILL"
        }
    }
}

// Avoid pulling in the libc crate for one call.
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
    #[link_name = "setsid"]
    fn libc_setsid() -> i32;
}
