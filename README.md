# Klippit — scaffold

A frame-accurate clip/GIF/screenshot exporter, triggered by a keypress
while watching in mpv. Panel UI ported from Sakuga Enhancer's design
system; backend uses native `ffmpeg`/`ffprobe` instead of `ffmpeg.wasm`.

**Status: scaffold, not yet built or run.** This container has no display
and no Rust/Tauri toolchain installed, so none of this has been compiled
or visually checked — treat it as a structured starting point, not a
finished build. Everything below is real, working logic *conceptually*
(the panel HTML/CSS/JS runs fine standalone in a browser today), but the
Tauri↔JS wiring and the mpv↔app handoff are stubbed with `TODO(tauri)`
comments where they need your machine to actually finish them.

## What works right now, standalone

Open `src/index.html` directly in a browser. The whole panel is
interactive: format/subtitle/mode toggles, the frame bar, draggable trim
handles, keyboard nudging (`,` / `.`, arrow keys on a focused handle,
`Enter`/`Escape`). It falls back to dev-preview values since there's no
mpv or Tauri backing it yet — `Export` logs its params to the console
instead of calling ffmpeg.

## What's stubbed and needs wiring

- **`app.js`** — every `TODO(tauri)` marks a spot that currently fakes
  something the real Tauri APIs would do: the save-folder dialog,
  `invoke('get_video_metadata', …)`, `invoke('export_clip', …)`, and
  closing the window. These are one-line swaps once you scaffold this with
  `tauri init` (or drop these files into that structure) and `window.__TAURI__`
  is actually present.
- **`src-tauri/src/main.rs`** — the real logic: ffprobe for metadata,
  ffmpeg for MP4 (quality mode + 2-pass target-size mode) and GIF
  (palette gen/use, with iterative width/fps backoff in target-size mode)
  export, subtitle burn-in with PTS-aligned trimming. This hasn't been
  compiled — check argument syntax against your installed ffmpeg version,
  and confirm `-ss` before `-i` (input seek) lands close enough to frame-
  accurate for your source files; if not, switch to output-side `-ss`
  (slower, always accurate) for the final encode.
- **`mpv-scripts/clip-trigger.lua`** — `APP_PATH` is a placeholder. Also
  flagged inline: this always spawns a new app instance rather than
  messaging an already-open one. Worth fixing before this is a daily
  driver — the natural approach is a local socket the Tauri app listens on
  at startup, which this script checks before falling back to spawning.
- **Screenshots** — intentionally *not* routed through this app. Bind
  mpv's own `screenshot video` / `screenshot subtitles` commands to keys
  directly in your `input.conf`; `extract_frame` in `main.rs` is only a
  fallback for pulling a still outside of a live mpv session.

## Structure

```
src/                    panel UI — HTML/CSS/JS, opens standalone in a browser
src-tauri/              Tauri shell: Cargo.toml, tauri.conf.json, main.rs
mpv-scripts/            Lua trigger script for mpv's scripts directory
```

## Bundling ffmpeg (no separate install for end users)

By default this scaffold calls ffmpeg/ffprobe as **sidecar** binaries
bundled inside the app, not from your system PATH — so once you build a
release with `cargo tauri build`, that `.msi` never needs ffmpeg installed
separately, on your machine or anyone else's. Setting this up is a
one-time step:

1. Download static ffmpeg + ffprobe builds for your platform (Windows:
   the "essentials" or "full" build from gyan.dev's ffmpeg-builds page;
   macOS/Linux: evermeet.cx or the official ffmpeg.org static builds page).
2. Find your Rust target triple: run `rustc -Vv` and look at the `host:`
   line (e.g. `x86_64-pc-windows-msvc`).
3. Create `src-tauri/binaries/` and place the two binaries there, renamed
   to include that triple — Tauri's sidecar mechanism requires this exact
   naming:
   - `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`
   - `src-tauri/binaries/ffprobe-x86_64-pc-windows-msvc.exe`
   (swap the triple/extension for your platform)
4. `cargo tauri dev` / `cargo tauri build` will now pick them up
   automatically — `tauri.conf.json`'s `externalBin` entry already points
   at this folder.

If you'd rather not bundle them (smaller download, but back to requiring
ffmpeg on PATH like the original scaffold), that's a valid tradeoff too —
just swap `run_bin`'s sidecar calls in `main.rs` back to
`std::process::Command::new("ffmpeg")`/`("ffprobe")`.

**Not yet verified against a real build**: the `tauri-plugin-shell`
sidecar API (`app.shell().sidecar(name).args(...).output().await`) is
written from the documented v2 pattern but hasn't been compiled in this
environment — method names have shifted slightly across 2.x point
releases, so if `cargo build` complains about `ShellExt` or `.sidecar(...)`,
check that crate's current docs for the exact call shape.

## Suggested next steps, in order

1. Run `src/index.html` in a browser and sanity-check the interaction
   model (frame bar, drag handles, keyboard) feels right before touching
   Rust at all — this is the part most worth iterating on by feel.
2. Scaffold a real Tauri project (`cargo install tauri-cli`, `tauri init`)
   and merge these files in; wire the `TODO(tauri)` spots in `app.js`.
3. Test `export_clip` against a real file from the command line first
   (call the ffmpeg args `main.rs` builds, by hand, before wiring the UI
   to them) — cheaper to debug ffmpeg syntax outside the GUI loop.
4. Point `clip-trigger.lua`'s `APP_PATH` at the built binary, drop it in
   mpv's `scripts/` directory, and test the handoff end to end.
5. Come back to single-instance messaging once the core loop works.
