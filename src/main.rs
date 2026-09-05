//! game-box — a TUI game manager for wine and proton.

mod app;
mod install;
mod launch;
mod library;
mod model;
mod runners;
mod scan;
mod sysmon;
mod theme;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;

const TICK: Duration = Duration::from_millis(100);

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_usage();
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("game-box {}", ui::VERSION);
            return Ok(());
        }
        Some("--scan") => {
            // Print what the wizard would offer, for debugging a stubborn title.
            let Some(dir) = args.get(1) else {
                eprintln!("usage: gbox --scan <directory>");
                std::process::exit(2);
            };
            let path = scan::expand_tilde(dir);
            let result = scan::scan_dir(&path, 8, 20);
            println!("prefix: {:?}", result.prefix);
            println!("built by: {:?}", result.runner_hint);
            for c in &result.candidates {
                println!("{:>5}  {:>8}  {}", c.score, model::human_size(c.size), c.rel);
            }
            return Ok(());
        }
        Some("--runners") => {
            for r in runners::discover() {
                println!("{:<8} {:<30} {}", r.kind.label(), r.name, r.bin.display());
            }
            return Ok(());
        }
        Some(other) if other.starts_with('-') => {
            eprintln!("unknown option: {}", other);
            print_usage();
            std::process::exit(2);
        }
        _ => {}
    }

    let mut app = App::new()?;
    // `gbox add <dir>` and `gbox install <exe>` drop straight into the wizard.
    if let Some(cmd @ ("add" | "install")) = args.first().map(String::as_str) {
        let path = args.get(1).cloned().unwrap_or_default();
        app.on_command(&format!("{} {}", cmd, path));
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn print_usage() {
    println!(
        "game-box {} — wine/proton game manager\n\n\
         usage:\n  \
           gbox                 launch the TUI\n  \
           gbox add [path]      launch and open the add-a-game wizard\n  \
           gbox install [exe]   launch and open the installer wizard\n  \
           gbox --runners       list detected wine/proton installations\n  \
           gbox --scan <dir>    show the executables the wizard would offer\n  \
           gbox --version\n",
        ui::VERSION
    );
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last = Instant::now();
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        let timeout = TICK.saturating_sub(last.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if last.elapsed() >= TICK {
            last = Instant::now();
            app.poll();
            app.poll_wizard();
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
