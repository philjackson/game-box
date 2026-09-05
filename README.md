# game-box

A TUI game manager for Wine and Proton. mutt's frame — sidebar, index, pager,
status line, `:` commands — with btop-flavoured meters.

```
gbox                 launch the TUI
gbox add [path]      launch straight into the add-a-game wizard
gbox --runners       list detected wine/proton installations
gbox --scan <dir>    show the executables the wizard would offer, with scores
```

## Two ways to add a game

`a` opens the wizard on a menu with both; `:add <dir>` and `:install <exe>`
(or `i`) skip straight to one.

In every path prompt, `Tab` and `^j` step forward through the matching entries
and `^k` steps back; the highlighted one is what is in the input, so you can
walk a directory listing without typing it out.

## 1. A game that already exists on disk

`a` opens the wizard. Point it at a directory — `/home/phil/extra/Games/iron-nest`
— and it will:

1. **find the prefix.** A directory counts as a WINEPREFIX when it holds
   `drive_c` plus `system.reg`/`user.reg`. Proton's compatdata layout
   (`<dir>/pfx`) and Lutris' flat layout (`pfx -> .`) both work, as does
   pointing at a game folder nested inside a prefix.
2. **rank the executables.** Installers, redist folders, crash handlers and
   the prefix's own `drive_c/windows` are filtered out; what is left is scored
   on name-vs-folder match, depth, size, and Unity/Unreal layout tells. You
   pick from the ranked list.
3. **preselect the runner** recorded in the prefix's `version` file, so an
   existing prefix is not rebuilt by a mismatched Proton.
4. **show the exact command** before anything is written.

Nothing is copied or moved: the library only stores paths.

## 2. Running a Windows installer

Pick a `setup.exe` (or `.msi`), pick a new empty directory for the prefix, a
runner and the prefix bitness, and game-box builds the prefix and runs the
installer inside it. The installer's own windows open on your desktop; the TUI
shows the phase, elapsed time and a live tail of the wine log.

- The prefix is created with `mscoree,mshtml=d`, so wine's mono/gecko prompts
  cannot silently block an install behind a dialog you never saw.
- Proton keeps its own `<dir>/pfx` layout; wine uses the directory directly and
  honours the win64/win32 choice.
- `x` cancels: the child is signalled *and* the prefix's wineserver is killed,
  because wine processes leave our process group behind.
- `Esc` leaves the install running in the background — the header keeps showing
  it, and `a`/`i` drops you back into the live view.
- When it finishes, game-box rescans the new prefix and you pick the installed
  game's executable exactly as above.

Same escalation applies to a running game: `x` asks it to quit, a second `x`
takes the whole prefix down through its wineserver.

## gamescope

`w` (or `:gamescope`) opens per-game settings: on/off, output resolution
(`-W`/`-H`), the resolution the game renders at (`-w`/`-h`), fullscreen /
borderless / windowed, an fps cap (`-r`), the upscaler (`-F linear|nearest|
fsr|nis|pixel`), `--mangoapp`, and a free-form field for anything else.

When it is on, gamescope becomes the program that gets executed and the runner
is what it hosts:

```
gamescope -W 2560 -H 1440 -w 1920 -h 1080 -f -r 60 -F fsr -- <proton|wine> <game.exe>
```

The editor previews that line as you type, and games launching under gamescope
carry a `g` flag in the index. Enabling it without gamescope on `$PATH` is
allowed but says so, and the launch fails with a clear message rather than
silently running without it.

## Keys

`j`/`k` move · `Enter` play · `x` kill · `a` add · `i` install · `R` runner ·
`w` gamescope · `C` winecfg ·
`o` log · `d` remove · `/` search · `s` sort · `b`/`v` panes · `:` command ·
`?` full help · `q` quit.

## Where things live

- library: `~/.local/share/game-box/library.json`
- launch logs: `~/.local/share/game-box/logs/<id>.log`
- install logs: `~/.local/share/game-box/logs/install-<prefix>.log`

Runners are discovered from `~/.local/share/Steam/compatibilitytools.d`,
Steam's `steamapps/common`, `~/.local/share/lutris/runners/wine`, and `wine`
on `$PATH`.

## Build

```
cargo build --release   # target/release/gbox
cargo test
```
