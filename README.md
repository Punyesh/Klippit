# Klippit — scaffold

A frame-accurate clip/GIF/screenshot exporter, triggered by a keypress
while watching in mpv. Panel UI ported from Sakuga Enhancer's design
system; backend uses native `ffmpeg`/`ffprobe` instead of `ffmpeg.wasm`.

**Status: scaffold, not yet built or run.** This container has no display
and no Rust/Tauri toolchain installed, so none of this has been compiled
or visually checked — treat it as a structured starting point, not a
finished build. The panel↔backend wiring (metadata, export, folder
picker, closing the window) is real, written-through code now, not stubs —
but it's untested against an actual compile, and the mpv↔app handoff
(reading `--init` and getting it into the webview) is still unbuilt.

## What works right now, standalone

Open `src/index.html` directly in a browser. The whole panel is
interactive: format/subtitle/mode toggles, the frame bar, draggable trim
handles, keyboard nudging (`,` / `.`, arrow keys on a focused handle,
`Enter`/`Escape`). With no `window.__TAURI__` present (i.e. a plain
browser tab), it falls back to dev-preview values and `Export` just logs
its params to the console — that fallback path is intentional, not a bug,
so you can iterate on the UI without a full Tauri build every time.

## What's real now vs. still unbuilt

- **`app.js` ↔ `main.rs`** — `get_video_metadata`, `export_clip`, the
  folder-picker dialog, and closing the window all call the real Tauri
  APIs now (`window.__TAURI__.core.invoke(...)`, `.dialog.open(...)`,
  `.window.getCurrentWindow().close()`), guarded by an
  `if (window.__TAURI__)` check so the browser-preview fallback above
  still works. **Not yet compiled or run** — first real test should be
  `cargo tauri dev` (see below).
- **ffmpeg/ffprobe as sidecars** — `main.rs` calls them as bundled
  binaries via `tauri-plugin-shell`, not from system PATH. See "Bundling
  ffmpeg" below — this is the one manual step that doesn't work without
  you supplying the actual binaries.
- **`capabilities/default.json`** — Tauri v2's permission system needs
  this to allow the dialog plugin and window-close call; included here,
  but the `$schema` path assumes a standard `tauri init` layout. If
  `cargo tauri dev` complains about it, that schema reference (not the
  permissions list itself) is the likely culprit — safe to delete that
  one line.
- **The mpv↔app handoff is still incomplete.** `clip-trigger.lua` passes
  `--init <json>` on the command line, but nothing in `main.rs` reads
  `std::env::args()` and injects it into the webview as
  `window.__KLIPPIT_INIT__` yet — so launching from mpv today still opens
  the panel in dev-preview mode rather than seeded with the real file/
  timestamp. That's the next real gap to close, once the build itself is
  confirmed working.
- **Loading the actual video file** — `app.js` currently sets
  `video.src = 'file://' + path`, which may get blocked by WebView2's
  cross-origin restrictions since the app itself runs under
  `tauri://localhost`. If the preview shows blank once a real file is
  wired up, switch to Tauri's asset protocol (`convertFileSrc` from
  `window.__TAURI__.core`) instead of a raw `file://` URL.
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
separately, on your machine or anyone else's.

**Easiest path:** run `scripts\setup-ffmpeg.ps1` from the repo root (or
anywhere — it locates the repo relative to its own location). It
downloads the current Windows ffmpeg build, detects your Rust target
triple, and places both binaries in `src-tauri/binaries/` with the exact
naming Tauri's sidecar mechanism requires. Re-run it any time to pick up
a newer ffmpeg release. These binaries are `.gitignore`'d — anyone
building this repo runs the script themselves rather than pulling ~80MB
of binaries out of git.

**Manual path**, if you'd rather not run a script or need a different
platform's build:

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

Either way, `cargo tauri dev` / `cargo tauri build` will pick them up
automatically — `tauri.conf.json`'s `externalBin` entry already points at
that folder.

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

1. `cargo tauri dev` first with placeholder ffmpeg binaries missing —
   confirm the window opens and the browser-preview UI behavior still
   holds inside the real webview before chasing ffmpeg issues.
2. Run `scripts\setup-ffmpeg.ps1` to get real ffmpeg/ffprobe binaries in
   place, pick a local video file's path manually in place of mpv for
   now, and test `get_video_metadata` + `export_clip` end to end.
3. If either command errors, test the equivalent ffmpeg/ffprobe args by
   hand in a terminal first — cheaper to debug ffmpeg syntax outside the
   GUI/IPC loop.
4. Wire the mpv↔app handoff: read `--init` in `main.rs`'s `main()`,
   inject it into the webview as `window.__KLIPPIT_INIT__` before the
   page loads. Point `clip-trigger.lua`'s `APP_PATH` at the built binary,
   drop the script in mpv's `scripts/` directory, and test end to end.
5. Come back to single-instance messaging once the core loop works.
6. `cargo tauri build` once everything above works in `dev` mode — that's
   what actually produces the distributable `.msi`/`.exe`.
