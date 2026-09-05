//! Building and spawning the wine/proton command line.

use std::fs::{self, File};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use anyhow::{anyhow, Result};
use chrono::Utc;

use crate::library;
use crate::model::{Game, Runner, RunnerKind};

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
    /// Set once the user has asked for a graceful stop, so a second press can
    /// escalate.
    pub kill_requested: bool,
}

/// The exact command that would be run, for the confirm screen and the log
/// header. Returned as (program, args, env).
pub fn build_command(game: &Game, runner: &Runner) -> Result<(PathBuf, Vec<String>, Vec<(String, String)>)> {
    let exe = game
        .exe
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not valid UTF-8"))?
        .to_string();

    let mut env: Vec<(String, String)> = Vec::new();
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

    Ok((runner.bin.clone(), args, env))
}

/// Render the command as a copy-pasteable shell line.
pub fn command_preview(game: &Game, runner: &Runner) -> String {
    match build_command(game, runner) {
        Ok((bin, args, env)) => {
            let mut s = String::new();
            for (k, v) in env {
                s.push_str(&format!("{}={} ", k, shell_quote(&v)));
            }
            s.push_str(&shell_quote(&bin.to_string_lossy()));
            for a in args {
                s.push(' ');
                s.push_str(&shell_quote(&a));
            }
            s
        }
        Err(e) => format!("(cannot launch: {})", e),
    }
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Spawn the game detached from our stdio, logging to the data dir.
pub fn launch(game: &Game, runner: &Runner) -> Result<Session> {
    if !game.exe.is_file() {
        return Err(anyhow!("executable is missing: {}", game.exe.display()));
    }
    let (bin, args, env) = build_command(game, runner)?;

    fs::create_dir_all(library::log_dir())?;
    let log_path = library::log_dir().join(format!("{}.log", game.id));
    let mut log = File::create(&log_path)?;
    {
        use std::io::Write;
        writeln!(log, "# game-box session {}", Utc::now().to_rfc3339())?;
        writeln!(log, "# runner {} ({})", runner.name, runner.kind.label())?;
        writeln!(log, "# {}", command_preview(game, runner))?;
        writeln!(log)?;
    }
    let err_log = log.try_clone()?;

    // Own process group, so we can signal the whole wine tree later and so
    // ^C in the TUI never reaches the game.
    let child = Command::new(&bin)
        .args(&args)
        .envs(env)
        .process_group(0)
        .current_dir(game.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err_log))
        .spawn()?;

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
        kill_requested: false,
    })
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
}
