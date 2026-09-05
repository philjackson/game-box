//! Application state and key handling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use anyhow::Result;
use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::launch::{self, Session};
use crate::library::Library;
use crate::model::*;
use crate::runners;
use crate::scan::{self, ScanResult};
use crate::sysmon::SysMon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Index,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Name,
    LastPlayed,
    Playtime,
    Size,
    Added,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::LastPlayed => "played",
            Sort::Playtime => "time",
            Sort::Size => "size",
            Sort::Added => "added",
        }
    }
    fn next(self) -> Sort {
        match self {
            Sort::Name => Sort::LastPlayed,
            Sort::LastPlayed => Sort::Playtime,
            Sort::Playtime => Sort::Size,
            Sort::Size => Sort::Added,
            Sort::Added => Sort::Name,
        }
    }
}

/// A sidebar entry — mutt's mailbox list, but for collections of games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collection {
    All,
    Running,
    Recent,
    Missing,
    Runner(String),
    Tag(String),
}

impl Collection {
    pub fn label(&self) -> String {
        match self {
            Collection::All => "all games".into(),
            Collection::Running => "running".into(),
            Collection::Recent => "recent".into(),
            Collection::Missing => "missing".into(),
            Collection::Runner(r) => r.clone(),
            Collection::Tag(t) => format!("#{}", t),
        }
    }
}

/// A single-line text prompt with a cursor, used by `:`, `/` and the wizard.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub label: String,
    pub value: String,
    pub cursor: usize,
    pub completions: Vec<String>,
    pub completion_idx: usize,
}

impl Prompt {
    pub fn new(label: &str, value: &str) -> Prompt {
        Prompt {
            label: label.to_string(),
            cursor: value.chars().count(),
            value: value.to_string(),
            completions: Vec::new(),
            completion_idx: 0,
        }
    }

    fn byte_at(&self, cursor: usize) -> usize {
        self.value
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    pub fn insert(&mut self, c: char) {
        let b = self.byte_at(self.cursor);
        self.value.insert(b, c);
        self.cursor += 1;
        self.completions.clear();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let b = self.byte_at(self.cursor - 1);
        self.value.remove(b);
        self.cursor -= 1;
        self.completions.clear();
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    pub fn kill(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.completions.clear();
    }
}

/// Where a new library entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddSource {
    /// Already unpacked somewhere on the filesystem.
    Existing,
    /// A Windows installer we run inside a fresh prefix.
    Installer,
}

impl AddSource {
    pub fn title(self) -> &'static str {
        match self {
            AddSource::Existing => "already on disk",
            AddSource::Installer => "run an installer",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            AddSource::Existing => {
                "Point at a folder that already holds the game and its wine prefix."
            }
            AddSource::Installer => {
                "Run a setup.exe in a brand new prefix, then register what it installed."
            }
        }
    }
}

/// Steps of the add-a-game wizard. Which ones are visited depends on the
/// source; the tail (scan → exe → name → confirm) is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddStep {
    /// Choose between the two ways in.
    Source,
    /// Existing: the directory holding the game.
    Path,
    /// Installer: the setup executable.
    Installer,
    /// Installer: where the new prefix goes.
    Dest,
    /// Installer: prefix bitness.
    Arch,
    Runner,
    /// Installer: the installer is running.
    Installing,
    Scanning,
    Exe,
    Name,
    Confirm,
}

#[derive(Debug)]
pub struct AddWizard {
    pub source: AddSource,
    pub source_idx: usize,
    pub step: AddStep,
    pub prompt: Prompt,
    /// Installer path, kept while the destination prompt has the input focus.
    pub installer: Option<PathBuf>,
    pub dest: Option<PathBuf>,
    pub arch: crate::install::Arch,
    pub scan: Option<ScanResult>,
    pub rx: Option<Receiver<ScanResult>>,
    pub exe_idx: usize,
    pub runner_idx: usize,
    pub name: String,
    pub error: Option<String>,
}

impl AddWizard {
    /// The source menu, which is where `a` lands.
    pub fn new() -> AddWizard {
        AddWizard {
            source: AddSource::Existing,
            source_idx: 0,
            step: AddStep::Source,
            prompt: Prompt::new("Directory", &default_games_dir()),
            installer: None,
            dest: None,
            arch: crate::install::Arch::Win64,
            scan: None,
            rx: None,
            exe_idx: 0,
            runner_idx: 0,
            name: String::new(),
            error: None,
        }
    }

    /// Skip the menu: `:add <path>` goes straight to the directory prompt.
    pub fn existing(initial: &str) -> AddWizard {
        let mut w = AddWizard::new();
        w.source = AddSource::Existing;
        w.source_idx = 0;
        w.step = AddStep::Path;
        w.prompt = Prompt::new("Directory", initial);
        w
    }

    /// Skip the menu: `:install <exe>` goes straight to the installer prompt.
    pub fn installer(initial: &str) -> AddWizard {
        let mut w = AddWizard::new();
        w.source = AddSource::Installer;
        w.source_idx = 1;
        w.step = AddStep::Installer;
        w.prompt = Prompt::new("Installer", initial);
        w
    }

    /// Re-enter a wizard whose install is already running in the background.
    pub fn resuming(job: &crate::install::InstallJob) -> AddWizard {
        let mut w = AddWizard::new();
        w.source = AddSource::Installer;
        w.source_idx = 1;
        w.step = AddStep::Installing;
        w.installer = Some(job.installer.clone());
        w.dest = Some(job.dest.clone());
        w.arch = job.arch;
        w
    }

    pub fn chosen_exe(&self) -> Option<PathBuf> {
        self.scan
            .as_ref()
            .and_then(|s| s.candidates.get(self.exe_idx))
            .map(|c| c.path.clone())
    }

    /// The step to fall back to when the user backs out of the exe list.
    pub fn step_before_exe(&self) -> AddStep {
        match self.source {
            AddSource::Existing => AddStep::Path,
            AddSource::Installer => AddStep::Installing,
        }
    }
}

#[derive(Debug)]
pub enum ConfirmAction {
    Delete(String),
    Quit,
}

#[derive(Debug)]
pub enum Overlay {
    Help { scroll: u16 },
    Add(Box<AddWizard>),
    Confirm { message: String, action: ConfirmAction },
    RunnerPick { game_id: String, idx: usize },
    Log { title: String, lines: Vec<String>, scroll: usize },
}

#[derive(Debug)]
pub enum Mode {
    Normal,
    Command(Prompt),
    Search(Prompt),
    Overlay(Overlay),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Error,
}

pub struct App {
    pub lib: Library,
    pub runners: Vec<Runner>,
    pub sys: SysMon,
    pub sessions: Vec<Session>,
    /// game id -> (cpu%, rss bytes) for running sessions.
    pub proc_stats: HashMap<String, (f32, u64)>,
    pub collections: Vec<Collection>,
    pub collection_idx: usize,
    pub query: String,
    pub view: Vec<usize>,
    pub cursor: usize,
    pub focus: Focus,
    pub mode: Mode,
    pub sort: Sort,
    pub sort_rev: bool,
    pub show_sidebar: bool,
    pub show_preview: bool,
    pub detail_scroll: u16,
    /// First visible row of the index; kept across frames so scrolling is
    /// sticky rather than re-centring on every redraw.
    pub index_offset: usize,
    pub message: Option<(String, MsgKind)>,
    pub message_tick: u64,
    pub tick: u64,
    pub should_quit: bool,
    /// At most one installer runs at a time. It outlives the wizard overlay so
    /// the user can close it and carry on browsing.
    pub install: Option<crate::install::InstallJob>,
    install_announced: bool,
    size_rx: Option<Receiver<(String, u64)>>,
}

impl App {
    pub fn new() -> Result<App> {
        let lib = Library::load()?;
        let mut app = App {
            lib,
            runners: runners::discover(),
            sys: SysMon::new(),
            sessions: Vec::new(),
            proc_stats: HashMap::new(),
            collections: Vec::new(),
            collection_idx: 0,
            query: String::new(),
            view: Vec::new(),
            cursor: 0,
            focus: Focus::Index,
            mode: Mode::Normal,
            sort: Sort::LastPlayed,
            sort_rev: false,
            show_sidebar: true,
            show_preview: true,
            detail_scroll: 0,
            index_offset: 0,
            message: None,
            message_tick: 0,
            tick: 0,
            should_quit: false,
            install: None,
            install_announced: false,
            size_rx: None,
        };
        app.rebuild_collections();
        app.refresh_view();
        app.spawn_size_scan();
        Ok(app)
    }

    // ---- state derivation -------------------------------------------------

    pub fn rebuild_collections(&mut self) {
        let mut cols = vec![
            Collection::All,
            Collection::Running,
            Collection::Recent,
        ];
        if self.lib.games.iter().any(|g| !g.exists()) {
            cols.push(Collection::Missing);
        }
        let mut runner_names: Vec<String> =
            self.lib.games.iter().map(|g| g.runner.name.clone()).collect();
        runner_names.sort();
        runner_names.dedup();
        cols.extend(runner_names.into_iter().map(Collection::Runner));

        let mut tags: Vec<String> =
            self.lib.games.iter().flat_map(|g| g.tags.iter().cloned()).collect();
        tags.sort();
        tags.dedup();
        cols.extend(tags.into_iter().map(Collection::Tag));

        self.collection_idx = self.collection_idx.min(cols.len().saturating_sub(1));
        self.collections = cols;
    }

    pub fn collection(&self) -> Collection {
        self.collections
            .get(self.collection_idx)
            .cloned()
            .unwrap_or(Collection::All)
    }

    pub fn count_in(&self, col: &Collection) -> usize {
        self.lib.games.iter().filter(|g| self.matches_collection(g, col)).count()
    }

    fn matches_collection(&self, g: &Game, col: &Collection) -> bool {
        match col {
            Collection::All => true,
            Collection::Running => self.is_running(&g.id),
            Collection::Recent => g.last_played.is_some(),
            Collection::Missing => !g.exists(),
            Collection::Runner(r) => &g.runner.name == r,
            Collection::Tag(t) => g.tags.contains(t),
        }
    }

    pub fn refresh_view(&mut self) {
        let col = self.collection();
        let q = self.query.to_ascii_lowercase();
        let mut idx: Vec<usize> = self
            .lib
            .games
            .iter()
            .enumerate()
            .filter(|(_, g)| self.matches_collection(g, &col))
            .filter(|(_, g)| {
                q.is_empty()
                    || g.name.to_ascii_lowercase().contains(&q)
                    || g.runner.name.to_ascii_lowercase().contains(&q)
                    || g.exe.to_string_lossy().to_ascii_lowercase().contains(&q)
                    || g.tags.iter().any(|t| t.to_ascii_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect();

        let sort = self.sort;
        let games = &self.lib.games;
        idx.sort_by(|&a, &b| {
            let (ga, gb) = (&games[a], &games[b]);
            match sort {
                Sort::Name => ga.name.to_ascii_lowercase().cmp(&gb.name.to_ascii_lowercase()),
                Sort::LastPlayed => gb.last_played.cmp(&ga.last_played),
                Sort::Playtime => gb.playtime_secs.cmp(&ga.playtime_secs),
                Sort::Size => gb.size_bytes.unwrap_or(0).cmp(&ga.size_bytes.unwrap_or(0)),
                Sort::Added => gb.added.cmp(&ga.added),
            }
        });
        if self.sort_rev {
            idx.reverse();
        }
        self.view = idx;
        if self.cursor >= self.view.len() {
            self.cursor = self.view.len().saturating_sub(1);
        }
    }

    pub fn selected(&self) -> Option<&Game> {
        self.view.get(self.cursor).map(|&i| &self.lib.games[i])
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected().map(|g| g.id.clone())
    }

    pub fn game_mut(&mut self, id: &str) -> Option<&mut Game> {
        self.lib.games.iter_mut().find(|g| g.id == id)
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.sessions.iter().any(|s| s.game_id == id)
    }

    pub fn session_for(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.game_id == id)
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), MsgKind::Info));
        self.message_tick = self.tick;
    }

    pub fn fail(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), MsgKind::Error));
        self.message_tick = self.tick;
    }

    // ---- background work --------------------------------------------------

    /// Size up every prefix we do not have a cached number for.
    pub fn spawn_size_scan(&mut self) {
        let pending: Vec<(String, PathBuf)> = self
            .lib
            .games
            .iter()
            .filter(|g| g.size_bytes.is_none())
            .map(|g| (g.id.clone(), g.prefix.clone()))
            .collect();
        if pending.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (id, path) in pending {
                let size = scan::dir_size(&path, 400_000);
                if tx.send((id, size)).is_err() {
                    return;
                }
            }
        });
        self.size_rx = Some(rx);
    }

    /// Called once per UI tick: reap finished games, resample meters.
    pub fn poll(&mut self) {
        self.tick += 1;
        if self.tick % 5 == 0 {
            self.sys.tick();
        }

        if let Some(rx) = &self.size_rx {
            let mut got = false;
            let updates: Vec<(String, u64)> = rx.try_iter().collect();
            for (id, size) in updates {
                if let Some(g) = self.lib.games.iter_mut().find(|g| g.id == id) {
                    g.size_bytes = Some(size);
                    got = true;
                }
            }
            if got {
                let _ = self.lib.save();
                self.refresh_view();
            }
        }

        // An install that finished while its overlay was closed still gets
        // announced, once.
        if let Some(job) = &self.install {
            if !job.is_live() && !self.install_announced {
                let phase = job.phase();
                self.install_announced = true;
                match phase {
                    crate::install::Phase::Done { code: 0 } => {
                        self.note("installer finished — press 'a' to register the game")
                    }
                    crate::install::Phase::Done { code } => self.note(format!(
                        "installer exited with status {} — press 'a' to look at it",
                        code
                    )),
                    crate::install::Phase::Cancelled => self.note("install cancelled"),
                    other => self.fail(other.label()),
                }
            }
        }

        // Sample the running games, then reap the ones that exited.
        let live: Vec<(String, u32)> =
            self.sessions.iter().map(|s| (s.game_id.clone(), s.pid)).collect();
        for (id, pid) in live {
            if let Some(stats) = self.sys.sample_proc(pid) {
                self.proc_stats.insert(id, stats);
            }
        }

        let mut finished: Vec<(String, u64, u32)> = Vec::new();
        self.sessions.retain_mut(|s| match s.child.try_wait() {
            Ok(Some(_)) => {
                finished.push((s.game_id.clone(), s.started.elapsed().as_secs(), s.pid));
                false
            }
            _ => true,
        });
        for (id, secs, pid) in finished {
            self.sys.forget(pid);
            self.proc_stats.remove(&id);
            if let Some(g) = self.lib.games.iter_mut().find(|g| g.id == id) {
                g.playtime_secs += secs;
                g.last_played = Some(Utc::now());
                g.sessions.push(((secs + 59) / 60) as u32);
                if g.sessions.len() > 64 {
                    let cut = g.sessions.len() - 64;
                    g.sessions.drain(0..cut);
                }
                let name = g.name.clone();
                let _ = self.lib.save();
                self.note(format!("{} exited after {}", name, human_duration(secs)));
            }
            self.rebuild_collections();
            self.refresh_view();
        }
    }

    // ---- actions ----------------------------------------------------------

    pub fn play_selected(&mut self) {
        let Some(id) = self.selected_id() else {
            self.fail("no game selected");
            return;
        };
        if self.is_running(&id) {
            self.fail("already running");
            return;
        }
        let Some(game) = self.lib.games.iter().find(|g| g.id == id).cloned() else { return };
        let Some(runner) = runners::resolve(&self.runners, &game.runner) else {
            self.fail(format!("runner {} is not installed", game.runner.name));
            return;
        };
        let runner = runner.clone();
        match launch::launch(&game, &runner) {
            Ok(session) => {
                self.note(format!("launched {} with {}", game.name, runner.name));
                self.sessions.push(session);
                self.rebuild_collections();
                self.refresh_view();
            }
            Err(e) => self.fail(format!("launch failed: {}", e)),
        }
    }

    pub fn kill_selected(&mut self) {
        let Some(id) = self.selected_id() else { return };
        match self.sessions.iter_mut().find(|s| s.game_id == id) {
            Some(s) => {
                let what = launch::terminate(s);
                self.note(what);
            }
            None => self.fail("that game is not running"),
        }
    }

    pub fn delete(&mut self, id: &str) {
        if let Some(pos) = self.lib.games.iter().position(|g| g.id == id) {
            let name = self.lib.games[pos].name.clone();
            self.lib.games.remove(pos);
            let _ = self.lib.save();
            self.rebuild_collections();
            self.refresh_view();
            self.note(format!("removed {} from the library (files untouched)", name));
        }
    }

    pub fn open_log(&mut self) {
        let Some(g) = self.selected() else { return };
        let path = crate::library::log_dir().join(format!("{}.log", g.id));
        let title = format!("{} — {}", g.name, path.display());
        match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let lines: Vec<String> = raw.lines().map(str::to_string).collect();
                self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll: 0 });
            }
            Err(_) => self.fail("no log yet — play the game first"),
        }
    }

    pub fn run_winecfg(&mut self) {
        let Some(g) = self.selected().cloned() else { return };
        let Some(runner) = runners::resolve(&self.runners, &g.runner).cloned() else {
            self.fail("runner not installed");
            return;
        };
        let bin = match runner.kind {
            RunnerKind::Wine => runner.bin.with_file_name("winecfg"),
            RunnerKind::Proton => runner
                .bin
                .parent()
                .map(|p| p.join("files/bin/winecfg"))
                .unwrap_or_default(),
        };
        if !bin.is_file() {
            self.fail(format!("winecfg not found at {}", bin.display()));
            return;
        }
        let prefix = if runner.kind == RunnerKind::Proton {
            g.compat_data_path().join("pfx")
        } else {
            g.prefix.clone()
        };
        match std::process::Command::new(&bin)
            .env("WINEPREFIX", &prefix)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.note(format!("winecfg on {}", prefix.display())),
            Err(e) => self.fail(format!("winecfg: {}", e)),
        }
    }

    /// Commit a finished wizard to the library.
    pub fn add_from_wizard(&mut self, w: &AddWizard) -> Result<(), String> {
        let scan = w.scan.as_ref().ok_or("nothing scanned")?;
        let exe = w.chosen_exe().ok_or("no executable chosen")?;
        let prefix = scan
            .prefix
            .clone()
            .ok_or("no wine prefix found for that directory")?;
        let runner = self
            .runners
            .get(w.runner_idx)
            .ok_or("no runner available — install proton or wine")?;
        let name = if w.name.trim().is_empty() {
            titleize(&exe.file_stem().unwrap_or_default().to_string_lossy())
        } else {
            w.name.trim().to_string()
        };
        if self.lib.games.iter().any(|g| g.exe == exe) {
            return Err("that executable is already in the library".into());
        }
        let id = self.lib.unique_id(&slugify(&name));
        let game = Game {
            id: id.clone(),
            name: name.clone(),
            exe,
            prefix,
            runner: RunnerRef::of(runner),
            args: Vec::new(),
            env: Default::default(),
            working_dir: None,
            tags: Vec::new(),
            notes: String::new(),
            added: Utc::now(),
            last_played: None,
            playtime_secs: 0,
            sessions: Vec::new(),
            size_bytes: None,
        };
        self.lib.games.push(game);
        self.lib.save().map_err(|e| e.to_string())?;
        self.rebuild_collections();
        self.refresh_view();
        self.spawn_size_scan();
        if let Some(pos) = self.view.iter().position(|&i| self.lib.games[i].id == id) {
            self.cursor = pos;
        }
        self.note(format!("added {}", name));
        Ok(())
    }

    // ---- input ------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        // Take the mode out so handlers can own their state.
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Normal => self.key_normal(key),
            Mode::Command(p) => self.key_command(key, p),
            Mode::Search(p) => self.key_search(key, p),
            Mode::Overlay(o) => self.key_overlay(key, o),
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => {
                if self.sessions.is_empty() {
                    self.should_quit = true;
                } else {
                    self.mode = Mode::Overlay(Overlay::Confirm {
                        message: format!(
                            "{} game(s) still running. Quit anyway? They keep running.",
                            self.sessions.len()
                        ),
                        action: ConfirmAction::Quit,
                    });
                }
            }
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('d') if ctrl => self.move_cursor(10),
            KeyCode::Char('u') if ctrl => self.move_cursor(-10),
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.mode = Mode::Overlay(Overlay::Help { scroll: 0 })
            }
            KeyCode::Char(':') => self.mode = Mode::Command(Prompt::new(":", "")),
            KeyCode::Char('/') => self.mode = Mode::Search(Prompt::new("/", &self.query.clone())),
            KeyCode::Char('a') => {
                self.mode = Mode::Overlay(Overlay::Add(Box::new(AddWizard::new())))
            }
            KeyCode::Char('i') => {
                let w = match &self.install {
                    // Rejoin an install already in flight rather than refusing.
                    Some(job) if job.is_live() || job.succeeded() => AddWizard::resuming(job),
                    _ => AddWizard::installer(&default_downloads_dir()),
                };
                self.mode = Mode::Overlay(Overlay::Add(Box::new(w)));
            }
            KeyCode::Char('d') => {
                if let Some(g) = self.selected() {
                    let (id, name) = (g.id.clone(), g.name.clone());
                    self.mode = Mode::Overlay(Overlay::Confirm {
                        message: format!("Remove \"{}\" from the library? Game files stay on disk.", name),
                        action: ConfirmAction::Delete(id),
                    });
                }
            }
            KeyCode::Char('R') => {
                if let Some(g) = self.selected() {
                    let id = g.id.clone();
                    let idx = self
                        .runners
                        .iter()
                        .position(|r| r.name == g.runner.name)
                        .unwrap_or(0);
                    self.mode = Mode::Overlay(Overlay::RunnerPick { game_id: id, idx });
                }
            }
            KeyCode::Enter | KeyCode::Char('p') => self.play_selected(),
            KeyCode::Char('x') | KeyCode::Char('K') => self.kill_selected(),
            KeyCode::Char('o') => self.open_log(),
            KeyCode::Char('C') => self.run_winecfg(),
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.refresh_view();
                self.note(format!("sort: {}", self.sort.label()));
            }
            KeyCode::Char('S') => {
                self.sort_rev = !self.sort_rev;
                self.refresh_view();
            }
            KeyCode::Char('b') => self.show_sidebar = !self.show_sidebar,
            KeyCode::Char('v') => self.show_preview = !self.show_preview,
            KeyCode::Char('$') => {
                self.runners = runners::discover();
                for g in &mut self.lib.games {
                    g.size_bytes = None;
                }
                self.spawn_size_scan();
                self.rebuild_collections();
                self.refresh_view();
                self.note("rescanning prefixes and runners");
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Index,
                    Focus::Index if self.show_preview => Focus::Detail,
                    _ => {
                        if self.show_sidebar {
                            Focus::Sidebar
                        } else {
                            Focus::Index
                        }
                    }
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Detail => Focus::Index,
                    Focus::Index if self.show_sidebar => Focus::Sidebar,
                    _ => Focus::Detail,
                }
            }
            KeyCode::Esc => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.refresh_view();
                    self.note("search cleared");
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::Char('g') | KeyCode::Home => self.set_cursor(0),
            KeyCode::Char('G') | KeyCode::End => self.set_cursor(isize::MAX),
            _ => {}
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        match self.focus {
            Focus::Sidebar => {
                let n = self.collections.len() as isize;
                if n == 0 {
                    return;
                }
                let next = (self.collection_idx as isize + delta).clamp(0, n - 1);
                self.collection_idx = next as usize;
                self.cursor = 0;
                self.detail_scroll = 0;
                self.refresh_view();
            }
            Focus::Detail => {
                self.detail_scroll = (self.detail_scroll as isize + delta).max(0) as u16;
            }
            Focus::Index => self.set_cursor(self.cursor as isize + delta),
        }
    }

    fn set_cursor(&mut self, to: isize) {
        if self.view.is_empty() {
            self.cursor = 0;
            return;
        }
        let max = self.view.len() as isize - 1;
        self.cursor = to.clamp(0, max) as usize;
        self.detail_scroll = 0;
    }

    fn key_command(&mut self, key: KeyEvent, mut p: Prompt) {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                let line = p.value.clone();
                self.run_command(&line);
            }
            KeyCode::Backspace => {
                p.backspace();
                self.mode = Mode::Command(p);
            }
            KeyCode::Left => {
                p.left();
                self.mode = Mode::Command(p);
            }
            KeyCode::Right => {
                p.right();
                self.mode = Mode::Command(p);
            }
            KeyCode::Char(c) => {
                p.insert(c);
                self.mode = Mode::Command(p);
            }
            _ => self.mode = Mode::Command(p),
        }
    }

    fn key_search(&mut self, key: KeyEvent, mut p: Prompt) {
        match key.code {
            KeyCode::Esc => {
                self.query.clear();
                self.refresh_view();
            }
            KeyCode::Enter => {
                self.query = p.value.clone();
                self.refresh_view();
                if self.view.is_empty() {
                    self.fail(format!("no games match \"{}\"", self.query));
                }
            }
            KeyCode::Backspace => {
                p.backspace();
                self.query = p.value.clone();
                self.refresh_view();
                self.mode = Mode::Search(p);
            }
            KeyCode::Char(c) => {
                p.insert(c);
                self.query = p.value.clone();
                self.refresh_view();
                self.mode = Mode::Search(p);
            }
            _ => self.mode = Mode::Search(p),
        }
    }

    /// Run a `:` command line. Public so `gbox add <path>` can reuse it.
    pub fn on_command(&mut self, line: &str) {
        self.run_command(line);
    }

    fn run_command(&mut self, line: &str) {
        let line = line.trim();
        let (cmd, rest) = match line.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (line, ""),
        };
        match cmd {
            "" => {}
            "q" | "quit" | "exit" => self.should_quit = true,
            "h" | "help" => self.mode = Mode::Overlay(Overlay::Help { scroll: 0 }),
            "a" | "add" => {
                let start = if rest.is_empty() { default_games_dir() } else { rest.to_string() };
                self.mode = Mode::Overlay(Overlay::Add(Box::new(AddWizard::existing(&start))));
            }
            "i" | "install" => {
                let start = if rest.is_empty() { default_downloads_dir() } else { rest.to_string() };
                let w = match &self.install {
                    Some(job) if job.is_live() || job.succeeded() => AddWizard::resuming(job),
                    _ => AddWizard::installer(&start),
                };
                self.mode = Mode::Overlay(Overlay::Add(Box::new(w)));
            }
            "d" | "delete" | "rm" => {
                if let Some(id) = self.selected_id() {
                    self.delete(&id);
                }
            }
            "play" | "run" => self.play_selected(),
            "kill" | "stop" => self.kill_selected(),
            "log" => self.open_log(),
            "winecfg" => self.run_winecfg(),
            "runner" => {
                if let Some(g) = self.selected() {
                    let id = g.id.clone();
                    let idx = self.runners.iter().position(|r| r.name == g.runner.name).unwrap_or(0);
                    self.mode = Mode::Overlay(Overlay::RunnerPick { game_id: id, idx });
                }
            }
            "tag" => {
                if let Some(id) = self.selected_id() {
                    let tags: Vec<String> =
                        rest.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                    if let Some(g) = self.game_mut(&id) {
                        g.tags = tags;
                    }
                    let _ = self.lib.save();
                    self.rebuild_collections();
                    self.refresh_view();
                }
            }
            "rename" => {
                if rest.is_empty() {
                    self.fail("usage: :rename <new name>");
                } else if let Some(id) = self.selected_id() {
                    if let Some(g) = self.game_mut(&id) {
                        g.name = rest.to_string();
                    }
                    let _ = self.lib.save();
                    self.refresh_view();
                }
            }
            "sort" => match rest {
                "name" => self.sort = Sort::Name,
                "played" => self.sort = Sort::LastPlayed,
                "time" => self.sort = Sort::Playtime,
                "size" => self.sort = Sort::Size,
                "added" => self.sort = Sort::Added,
                other => {
                    self.fail(format!("unknown sort key \"{}\"", other));
                    return;
                }
            },
            other => self.fail(format!("unknown command \"{}\" — try :help", other)),
        }
        self.refresh_view();
    }

    fn key_overlay(&mut self, key: KeyEvent, overlay: Overlay) {
        match overlay {
            Overlay::Help { mut scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    scroll = scroll.saturating_add(1);
                    self.mode = Mode::Overlay(Overlay::Help { scroll });
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    scroll = scroll.saturating_sub(1);
                    self.mode = Mode::Overlay(Overlay::Help { scroll });
                }
                _ => self.mode = Mode::Overlay(Overlay::Help { scroll }),
            },
            Overlay::Confirm { message, action } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => match action {
                    ConfirmAction::Delete(id) => self.delete(&id),
                    ConfirmAction::Quit => self.should_quit = true,
                },
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {}
                _ => self.mode = Mode::Overlay(Overlay::Confirm { message, action }),
            },
            Overlay::Log { title, lines, mut scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    scroll = (scroll + 1).min(lines.len().saturating_sub(1));
                    self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll });
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    scroll = scroll.saturating_sub(1);
                    self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll });
                }
                KeyCode::Char('G') | KeyCode::End => {
                    scroll = lines.len().saturating_sub(1);
                    self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll });
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll: 0 });
                }
                _ => self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll }),
            },
            Overlay::RunnerPick { game_id, mut idx } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {}
                KeyCode::Enter => {
                    if let Some(r) = self.runners.get(idx).cloned() {
                        if let Some(g) = self.game_mut(&game_id) {
                            g.runner = RunnerRef::of(&r);
                        }
                        let _ = self.lib.save();
                        self.rebuild_collections();
                        self.refresh_view();
                        self.note(format!("runner set to {}", r.name));
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    idx = (idx + 1).min(self.runners.len().saturating_sub(1));
                    self.mode = Mode::Overlay(Overlay::RunnerPick { game_id, idx });
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    idx = idx.saturating_sub(1);
                    self.mode = Mode::Overlay(Overlay::RunnerPick { game_id, idx });
                }
                _ => self.mode = Mode::Overlay(Overlay::RunnerPick { game_id, idx }),
            },
            Overlay::Add(w) => self.key_add(key, w),
        }
    }

    /// Shared editing keys for the wizard's single-line prompts. Returns true
    /// when the key was consumed here.
    fn prompt_key(p: &mut Prompt, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Backspace => p.backspace(),
            KeyCode::Left => p.left(),
            KeyCode::Right => p.right(),
            KeyCode::Home => p.home(),
            KeyCode::End => p.end(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => p.kill(),
            // Anything else with ctrl held is a binding, not text to type.
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => return false,
            KeyCode::Char(c) => p.insert(c),
            _ => return false,
        }
        true
    }

    /// Step through a path prompt's completions. `delta` is +1 for Tab / ^J and
    /// -1 for ^K; the first press builds the list, later ones move within it.
    fn complete_prompt(p: &mut Prompt, with_exes: bool, delta: isize) {
        if p.completions.is_empty() {
            p.completions = scan::complete_entries(&p.value, with_exes)
                .into_iter()
                .map(|path| {
                    let mut s = path.to_string_lossy().to_string();
                    if path.is_dir() {
                        s.push('/');
                    }
                    s
                })
                .collect();
            // Stepping backwards into a fresh list starts at the bottom.
            p.completion_idx = if delta < 0 { p.completions.len().saturating_sub(1) } else { 0 };
        } else {
            let n = p.completions.len() as isize;
            p.completion_idx = (((p.completion_idx as isize + delta) % n + n) % n) as usize;
        }
        let Some(c) = p.completions.get(p.completion_idx).cloned() else { return };
        // `insert`/`backspace` clear the completion list, so save it across
        // the edit that applies the current candidate.
        let saved = std::mem::take(&mut p.completions);
        let idx = p.completion_idx;
        p.value = c;
        p.end();
        p.completions = saved;
        p.completion_idx = idx;
    }

    fn begin_scan(w: &mut AddWizard, root: PathBuf) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(scan::scan_dir(&root, 8, 60));
        });
        w.rx = Some(rx);
        w.step = AddStep::Scanning;
    }

    fn key_add(&mut self, key: KeyEvent, mut w: Box<AddWizard>) {
        if key.code == KeyCode::Esc {
            // An install keeps running: it belongs to the app, not the overlay.
            if w.step == AddStep::Installing && self.install.as_ref().is_some_and(|j| j.is_live()) {
                self.note("install continues in the background — press 'a' to look in on it");
            }
            return; // drops the wizard, back to normal mode
        }
        w.error = None;
        match w.step {
            // -- choose how we are adding ---------------------------------
            AddStep::Source => {
                const SOURCES: [AddSource; 2] = [AddSource::Existing, AddSource::Installer];
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                        w.source_idx = (w.source_idx + 1) % SOURCES.len()
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        w.source_idx = (w.source_idx + SOURCES.len() - 1) % SOURCES.len()
                    }
                    KeyCode::Char('1') => w.source_idx = 0,
                    KeyCode::Char('2') => w.source_idx = 1,
                    KeyCode::Enter => {
                        w.source = SOURCES[w.source_idx];
                        match w.source {
                            AddSource::Existing => {
                                w.prompt = Prompt::new("Directory", &default_games_dir());
                                w.step = AddStep::Path;
                            }
                            AddSource::Installer => {
                                // Already installing? Drop back into the live view.
                                if let Some(job) = &self.install {
                                    if job.is_live() || job.succeeded() {
                                        w = Box::new(AddWizard::resuming(job));
                                        self.mode = Mode::Overlay(Overlay::Add(w));
                                        return;
                                    }
                                }
                                w.prompt = Prompt::new("Installer", &default_downloads_dir());
                                w.step = AddStep::Installer;
                            }
                        }
                    }
                    _ => {}
                }
            }

            // -- source: a directory that already holds the game -----------
            AddStep::Path => match key.code {
                KeyCode::Enter => {
                    let path = scan::expand_tilde(w.prompt.value.trim());
                    if !path.is_dir() {
                        w.error = Some(format!("{} is not a directory", path.display()));
                    } else {
                        Self::begin_scan(&mut w, path);
                    }
                }
                KeyCode::Tab => Self::complete_prompt(&mut w.prompt, false, 1),
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Self::complete_prompt(&mut w.prompt, false, 1)
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Self::complete_prompt(&mut w.prompt, false, -1)
                }
                _ => {
                    if !Self::prompt_key(&mut w.prompt, key) && key.code == KeyCode::Up {
                        w.step = AddStep::Source;
                    }
                }
            },

            // -- source: run an installer ----------------------------------
            AddStep::Installer => match key.code {
                KeyCode::Enter => {
                    let path = scan::expand_tilde(w.prompt.value.trim());
                    if !scan::is_windows_exe(&path) {
                        w.error = Some(format!("{} is not an .exe or .msi", path.display()));
                    } else {
                        // Suggest a prefix named after the installer.
                        let stem = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "game".into());
                        let suggestion = format!("{}{}", default_games_dir(), slugify(&stem));
                        w.installer = Some(path);
                        w.prompt = Prompt::new("Install into", &suggestion);
                        w.step = AddStep::Dest;
                    }
                }
                KeyCode::Tab => Self::complete_prompt(&mut w.prompt, true, 1),
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Self::complete_prompt(&mut w.prompt, true, 1)
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Self::complete_prompt(&mut w.prompt, true, -1)
                }
                _ => {
                    Self::prompt_key(&mut w.prompt, key);
                }
            },

            AddStep::Dest => match key.code {
                KeyCode::Enter => {
                    let path = scan::expand_tilde(w.prompt.value.trim());
                    if path.as_os_str().is_empty() {
                        w.error = Some("give the new prefix a directory".into());
                    } else if path.is_file() {
                        w.error = Some(format!("{} is a file", path.display()));
                    } else if path.is_dir()
                        && std::fs::read_dir(&path).map(|mut d| d.next().is_some()).unwrap_or(false)
                    {
                        w.error = Some(format!(
                            "{} is not empty — pick a fresh directory for the prefix",
                            path.display()
                        ));
                    } else {
                        w.dest = Some(path);
                        // Match the runner the user most likely wants: newest proton.
                        w.step = AddStep::Runner;
                    }
                }
                KeyCode::Tab => Self::complete_prompt(&mut w.prompt, false, 1),
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Self::complete_prompt(&mut w.prompt, false, 1)
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Self::complete_prompt(&mut w.prompt, false, -1)
                }
                _ => {
                    if !Self::prompt_key(&mut w.prompt, key) && key.code == KeyCode::Up {
                        w.prompt = Prompt::new(
                            "Installer",
                            &w.installer.clone().unwrap_or_default().to_string_lossy(),
                        );
                        w.step = AddStep::Installer;
                    }
                }
            },

            AddStep::Arch => match key.code {
                KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('k') | KeyCode::Up
                | KeyCode::Tab | KeyCode::Char(' ') => {
                    w.arch = match w.arch {
                        crate::install::Arch::Win64 => crate::install::Arch::Win32,
                        crate::install::Arch::Win32 => crate::install::Arch::Win64,
                    };
                }
                KeyCode::Backspace => w.step = AddStep::Runner,
                KeyCode::Enter => match self.start_install(&w) {
                    Ok(()) => w.step = AddStep::Installing,
                    Err(e) => w.error = Some(e),
                },
                _ => {}
            },

            AddStep::Installing => {
                let live = self.install.as_ref().is_some_and(|j| j.is_live());
                match key.code {
                    KeyCode::Char('x') if live => {
                        if let Some(job) = &self.install {
                            job.cancel();
                        }
                        self.note("cancelling the installer");
                    }
                    KeyCode::Enter if !live => {
                        // Installer done: look at what landed in the prefix.
                        match &self.install {
                            Some(job) if job.succeeded() => {
                                let dest = job.dest.clone();
                                Self::begin_scan(&mut w, dest);
                            }
                            Some(job) => {
                                w.error = Some(format!(
                                    "{} — nothing to register",
                                    job.phase().label()
                                ));
                            }
                            None => w.error = Some("no install is running".into()),
                        }
                    }
                    KeyCode::Char('o') => {
                        if let Some(job) = &self.install {
                            let lines = crate::install::tail(&job.log, 5000);
                            let title = job.log.display().to_string();
                            self.mode = Mode::Overlay(Overlay::Log { title, lines, scroll: 0 });
                            return;
                        }
                    }
                    _ => {}
                }
            }

            AddStep::Scanning => {}

            // -- shared tail: pick the exe, the runner, the name ------------
            AddStep::Exe => {
                let n = w.scan.as_ref().map(|s| s.candidates.len()).unwrap_or(0);
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => w.exe_idx = (w.exe_idx + 1).min(n.saturating_sub(1)),
                    KeyCode::Char('k') | KeyCode::Up => w.exe_idx = w.exe_idx.saturating_sub(1),
                    KeyCode::Char('g') => w.exe_idx = 0,
                    KeyCode::Char('G') => w.exe_idx = n.saturating_sub(1),
                    KeyCode::Backspace => w.step = w.step_before_exe(),
                    KeyCode::Enter => {
                        if n == 0 {
                            w.error = Some("no candidate executables found".into());
                        } else if w.source == AddSource::Installer {
                            // The runner was already chosen to build the prefix.
                            if w.name.is_empty() {
                                w.name = default_name(&w);
                            }
                            w.prompt = Prompt::new("Name", &w.name.clone());
                            w.step = AddStep::Name;
                        } else {
                            // Pre-select the runner the prefix was built with.
                            if let Some(hint) = w.scan.as_ref().and_then(|s| s.runner_hint.clone()) {
                                if let Some(i) = self.runners.iter().position(|r| r.name.starts_with(&hint)) {
                                    w.runner_idx = i;
                                }
                            }
                            w.step = AddStep::Runner;
                        }
                    }
                    _ => {}
                }
            }

            AddStep::Runner => {
                let n = self.runners.len();
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => w.runner_idx = (w.runner_idx + 1).min(n.saturating_sub(1)),
                    KeyCode::Char('k') | KeyCode::Up => w.runner_idx = w.runner_idx.saturating_sub(1),
                    KeyCode::Backspace => {
                        w.step = match w.source {
                            AddSource::Existing => AddStep::Exe,
                            AddSource::Installer => AddStep::Dest,
                        }
                    }
                    KeyCode::Enter => {
                        if n == 0 {
                            w.error = Some("no runners found — install proton or wine".into());
                        } else if w.source == AddSource::Installer {
                            w.step = AddStep::Arch;
                        } else {
                            if w.name.is_empty() {
                                w.name = default_name(&w);
                            }
                            w.prompt = Prompt::new("Name", &w.name.clone());
                            w.step = AddStep::Name;
                        }
                    }
                    _ => {}
                }
            }

            AddStep::Name => match key.code {
                KeyCode::Enter => {
                    w.name = w.prompt.value.trim().to_string();
                    w.step = AddStep::Confirm;
                }
                KeyCode::Backspace if w.prompt.value.is_empty() => {
                    w.step = match w.source {
                        AddSource::Existing => AddStep::Runner,
                        AddSource::Installer => AddStep::Exe,
                    }
                }
                _ => {
                    Self::prompt_key(&mut w.prompt, key);
                }
            },

            AddStep::Confirm => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => match self.add_from_wizard(&w) {
                    Ok(()) => {
                        // The install is registered; forget the finished job.
                        if w.source == AddSource::Installer {
                            self.install = None;
                        }
                        return;
                    }
                    Err(e) => {
                        w.error = Some(e);
                    }
                },
                KeyCode::Backspace | KeyCode::Char('n') => w.step = AddStep::Name,
                _ => {}
            },
        }
        self.mode = Mode::Overlay(Overlay::Add(w));
    }

    /// Kick off the installer described by the wizard.
    fn start_install(&mut self, w: &AddWizard) -> Result<(), String> {
        if self.install.as_ref().is_some_and(|j| j.is_live()) {
            return Err("an install is already running".into());
        }
        let installer = w.installer.clone().ok_or("no installer chosen")?;
        let dest = w.dest.clone().ok_or("no destination chosen")?;
        let runner = self
            .runners
            .get(w.runner_idx)
            .cloned()
            .ok_or("no runner available — install proton or wine")?;
        self.install = Some(crate::install::InstallJob::start(
            installer, dest, runner, w.arch,
        ));
        self.install_announced = false;
        Ok(())
    }

    /// Pump the wizard's background scan; called every tick.
    pub fn poll_wizard(&mut self) {
        let Mode::Overlay(Overlay::Add(w)) = &mut self.mode else { return };
        if w.step != AddStep::Scanning {
            return;
        }
        let Some(rx) = &w.rx else { return };
        if let Ok(result) = rx.try_recv() {
            w.rx = None;
            let back = w.step_before_exe();
            if result.prefix.is_none() {
                w.error = Some(match w.source {
                    AddSource::Existing => {
                        "no wine prefix here (need drive_c + system.reg, or a pfx/ subdir)".into()
                    }
                    AddSource::Installer => format!(
                        "the installer left no wine prefix in {} — check the log with 'o'",
                        result.root.display()
                    ),
                });
                w.step = back;
                return;
            }
            if result.candidates.is_empty() {
                w.error = Some(match w.source {
                    AddSource::Existing => "no .exe candidates found under that directory".into(),
                    AddSource::Installer => {
                        "the installer did not leave a launchable .exe behind".into()
                    }
                });
                w.step = back;
                return;
            }
            w.exe_idx = 0;
            w.scan = Some(result);
            w.step = AddStep::Exe;
        }
    }
}

fn default_name(w: &AddWizard) -> String {
    if let Some(scan) = &w.scan {
        let from_dir = scan
            .root
            .file_name()
            .map(|n| titleize(&n.to_string_lossy()))
            .unwrap_or_default();
        if !from_dir.is_empty() {
            return from_dir;
        }
    }
    w.chosen_exe()
        .map(|e| titleize(&e.file_stem().unwrap_or_default().to_string_lossy()))
        .unwrap_or_default()
}

/// Where installers are likely to be sitting.
fn default_downloads_dir() -> String {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    for candidate in ["Downloads", "downloads", "extra/Downloads"] {
        let p = home.join(candidate);
        if p.is_dir() {
            return format!("{}/", p.to_string_lossy());
        }
    }
    format!("{}/", home.to_string_lossy())
}

/// A sensible starting point for the path prompt.
fn default_games_dir() -> String {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    for candidate in ["extra/Games", "Games", "games"] {
        let p = home.join(candidate);
        if p.is_dir() {
            return format!("{}/", p.to_string_lossy());
        }
    }
    format!("{}/", home.to_string_lossy())
}
