//! Running a Windows installer inside a fresh wine or proton prefix.
//!
//! The whole job runs on a background thread so the TUI keeps redrawing; the
//! thread publishes its progress through `phase` and everything the installer
//! prints lands in a log file the UI tails.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::library;
use crate::model::{Runner, RunnerKind};

/// Prefix bitness. Only meaningful for wine — proton is 64-bit with a WoW64
/// layer, and ignores WINEARCH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Win64,
    Win32,
}

impl Arch {
    pub fn label(self) -> &'static str {
        match self {
            Arch::Win64 => "win64",
            Arch::Win32 => "win32",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Arch::Win64 => "64-bit — right for anything modern",
            Arch::Win32 => "32-bit — needed by some older installers",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Making the destination directory.
    Preparing,
    /// Creating the prefix (`wineboot` / proton's first run).
    Booting,
    /// The installer itself is on screen.
    Installing,
    Done { code: i32 },
    Failed(String),
    Cancelled,
}

impl Phase {
    pub fn is_live(&self) -> bool {
        matches!(self, Phase::Preparing | Phase::Booting | Phase::Installing)
    }

    pub fn label(&self) -> String {
        match self {
            Phase::Preparing => "preparing destination".into(),
            Phase::Booting => "creating the wine prefix".into(),
            Phase::Installing => "running the installer".into(),
            Phase::Done { code: 0 } => "installer finished".into(),
            Phase::Done { code } => format!("installer exited with status {}", code),
            Phase::Failed(e) => format!("failed: {}", e),
            Phase::Cancelled => "cancelled".into(),
        }
    }
}

/// A running (or finished) installation.
pub struct InstallJob {
    pub installer: PathBuf,
    pub dest: PathBuf,
    /// Where the WINEPREFIX ends up: `dest` for wine, `dest/pfx` for proton.
    pub prefix: PathBuf,
    pub runner: Runner,
    pub arch: Arch,
    pub log: PathBuf,
    pub started: Instant,
    phase: Arc<Mutex<Phase>>,
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl std::fmt::Debug for InstallJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallJob")
            .field("installer", &self.installer)
            .field("dest", &self.dest)
            .field("phase", &self.phase())
            .finish()
    }
}

impl InstallJob {
    pub fn phase(&self) -> Phase {
        self.phase.lock().map(|p| p.clone()).unwrap_or(Phase::Cancelled)
    }

    pub fn is_live(&self) -> bool {
        self.phase().is_live()
    }

    /// Did the installer complete in a way that is worth scanning for a game?
    pub fn succeeded(&self) -> bool {
        matches!(self.phase(), Phase::Done { .. })
    }

    /// Stop the installer and everything it spawned.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                // The installer is its own process group, so this reaches the
                // wineserver children too.
                unsafe {
                    kill(-(child.id() as i32), 15);
                }
                let _ = child.kill();
            }
        }
        // The wineserver and the processes it supervises live in their own
        // session, so the signal above never reaches them.
        if let Some(ws) = crate::runners::wineserver_for(&self.runner) {
            crate::runners::kill_prefix(&ws, &self.prefix);
        }
        if let Ok(mut p) = self.phase.lock() {
            if p.is_live() {
                *p = Phase::Cancelled;
            }
        }
    }

    /// Start the job. Returns immediately; watch [`InstallJob::phase`].
    pub fn start(installer: PathBuf, dest: PathBuf, runner: Runner, arch: Arch) -> InstallJob {
        let prefix = match runner.kind {
            // Proton insists on managing `<compat>/pfx` itself.
            RunnerKind::Proton => dest.join("pfx"),
            RunnerKind::Wine => dest.clone(),
        };
        let log = library::log_dir().join(format!(
            "install-{}.log",
            crate::model::slugify(&dest.file_name().unwrap_or_default().to_string_lossy())
        ));

        let job = InstallJob {
            installer: installer.clone(),
            dest: dest.clone(),
            prefix,
            runner: runner.clone(),
            arch,
            log: log.clone(),
            started: Instant::now(),
            phase: Arc::new(Mutex::new(Phase::Preparing)),
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
        };

        let phase = Arc::clone(&job.phase);
        let child_slot = Arc::clone(&job.child);
        let cancelled = Arc::clone(&job.cancelled);

        std::thread::spawn(move || {
            let worker = Worker { phase, child_slot, cancelled, log, dest, installer, runner, arch };
            worker.run();
        });

        job
    }
}

struct Worker {
    phase: Arc<Mutex<Phase>>,
    child_slot: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
    log: PathBuf,
    dest: PathBuf,
    installer: PathBuf,
    runner: Runner,
    arch: Arch,
}

impl Worker {
    fn set(&self, p: Phase) {
        if let Ok(mut guard) = self.phase.lock() {
            *guard = p;
        }
    }

    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn run(self) {
        if let Err(e) = fs::create_dir_all(&self.dest) {
            self.set(Phase::Failed(format!("cannot create {}: {}", self.dest.display(), e)));
            return;
        }
        if let Err(e) = fs::create_dir_all(library::log_dir()) {
            self.set(Phase::Failed(format!("cannot create the log directory: {}", e)));
            return;
        }
        if let Err(e) = File::create(&self.log) {
            self.set(Phase::Failed(format!("cannot write {}: {}", self.log.display(), e)));
            return;
        }
        self.say(&format!(
            "# game-box install\n# installer {}\n# destination {}\n# runner {} ({}, {})\n",
            self.installer.display(),
            self.dest.display(),
            self.runner.name,
            self.runner.kind.label(),
            self.arch.label(),
        ));

        // --- create the prefix -------------------------------------------
        // Non-fatal: several proton builds bootstrap the prefix on their first
        // real run anyway, so a failure here is a warning, not the end.
        self.set(Phase::Booting);
        // Wine's mono/gecko prompts are modal: without these overrides
        // `wineboot` sits forever behind a dialog the user may not even see.
        let (boot_bin, boot_args) = match self.runner.kind {
            RunnerKind::Proton => (self.runner.bin.clone(), vec!["run".into(), "wineboot".into(), "-u".into()]),
            RunnerKind::Wine => (self.runner.bin.clone(), vec!["wineboot".into(), "--init".into()]),
        };
        match self.spawn_and_wait_with(&boot_bin, &boot_args, &[("WINEDLLOVERRIDES", "mscoree,mshtml=d")]) {
            Ok(0) => self.say("\n# prefix ready\n"),
            Ok(code) => self.say(&format!("\n# wineboot exited {} — continuing anyway\n", code)),
            Err(e) => self.say(&format!("\n# wineboot could not run ({}) — continuing anyway\n", e)),
        }
        if self.stopped() {
            self.set(Phase::Cancelled);
            return;
        }

        // --- run the installer -------------------------------------------
        self.set(Phase::Installing);
        self.say(&format!("\n# launching {}\n\n", self.installer.display()));
        let args: Vec<String> = match self.runner.kind {
            RunnerKind::Proton => vec!["run".into(), self.installer.to_string_lossy().into()],
            RunnerKind::Wine => vec![self.installer.to_string_lossy().into()],
        };
        match self.spawn_and_wait(&self.runner.bin, &args) {
            Ok(code) => {
                if self.stopped() {
                    self.set(Phase::Cancelled);
                } else {
                    self.say(&format!("\n# installer exited with status {}\n", code));
                    self.set(Phase::Done { code });
                }
            }
            Err(e) => {
                self.say(&format!("\n# could not run the installer: {}\n", e));
                self.set(Phase::Failed(e));
            }
        }
    }

    /// Environment shared by the boot step and the installer itself.
    fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![
            // Keep `err:` and `warn:` — they are how a failed install explains
            // itself — but drop wine's very chatty fixme channel.
            ("WINEDEBUG".to_string(), "fixme-all".to_string()),
        ];
        match self.runner.kind {
            RunnerKind::Proton => {
                let steam = dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("/"))
                    .join(".local/share/Steam");
                env.push((
                    "STEAM_COMPAT_DATA_PATH".into(),
                    self.dest.to_string_lossy().into(),
                ));
                env.push((
                    "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                    steam.to_string_lossy().into(),
                ));
            }
            RunnerKind::Wine => {
                env.push(("WINEPREFIX".into(), self.dest.to_string_lossy().into()));
                env.push(("WINEARCH".into(), self.arch.label().to_string()));
            }
        }
        env
    }

    fn spawn_and_wait(&self, bin: &Path, args: &[String]) -> Result<i32, String> {
        self.spawn_and_wait_with(bin, args, &[])
    }

    fn spawn_and_wait_with(
        &self,
        bin: &Path,
        args: &[String],
        extra_env: &[(&str, &str)],
    ) -> Result<i32, String> {
        if self.stopped() {
            return Err("cancelled".into());
        }
        let log = OpenOptions::new()
            .append(true)
            .open(&self.log)
            .map_err(|e| e.to_string())?;
        let err_log = log.try_clone().map_err(|e| e.to_string())?;

        let mut env = self.env();
        env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())));

        let child = Command::new(bin)
            .args(args)
            .envs(env)
            .current_dir(
                self.installer
                    .parent()
                    .filter(|p| p.is_dir())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.dest.clone()),
            )
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log))
            .spawn()
            .map_err(|e| format!("{}: {}", bin.display(), e))?;

        // Publish the handle so a cancel can reach it, then wait.
        if let Ok(mut slot) = self.child_slot.lock() {
            *slot = Some(child);
        }
        let status = loop {
            let mut slot = match self.child_slot.lock() {
                Ok(s) => s,
                Err(_) => return Err("install thread poisoned".into()),
            };
            let Some(child) = slot.as_mut() else {
                return Err("cancelled".into());
            };
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    drop(slot);
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        if let Ok(mut slot) = self.child_slot.lock() {
            *slot = None;
        }
        Ok(status.code().unwrap_or(-1))
    }

    fn say(&self, text: &str) {
        if let Ok(mut f) = OpenOptions::new().append(true).open(&self.log) {
            let _ = f.write_all(text.as_bytes());
        }
    }
}

/// Read the last `max_lines` lines of a log without loading the whole file.
pub fn tail(path: &Path, max_lines: usize) -> Vec<String> {
    const WINDOW: u64 = 64 * 1024;
    let Ok(mut f) = File::open(path) else { return Vec::new() };
    let Ok(len) = f.seek(SeekFrom::End(0)) else { return Vec::new() };
    let start = len.saturating_sub(WINDOW);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    use std::io::Read;
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if start > 0 && !lines.is_empty() {
        // The first line is probably cut in half.
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(max_lines);
    lines.split_off(skip)
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
