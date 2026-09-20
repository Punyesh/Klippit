# Klippit — scaffold

A frame-accurate clip/GIF/screenshot exporter, triggered by a keypress
while watching in mpv. Panel UI ported from Sakuga Enhancer's design
system; backend uses native `ffmpeg`/`ffprobe` instead of `ffmpeg.wasm`.

**Status: in active development against a real Windows build.** Most of
the panel↔backend wiring (metadata, export, folder picker, opener
actions, closing the window) and the mpv↔app handoff (`--init` →
`window.__KLIPPIT_INIT__`) are now implemented and have been exercised
against real compiles — see the numbered list below for what's confirmed
working vs. still being debugged.

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
- ~~**The mpv↔app handoff is incomplete**~~ — implemented: `main.rs` now
  reads `--init <json>` from argv and injects it as
  `window.__KLIPPIT_INIT__` via `initialization_script`, which Tauri
  guarantees runs before `app.js`'s own code — no race condition between
  the two. The window itself is now built programmatically in `main.rs`'s
  `.setup()` rather than declared in `tauri.conf.json`, since
  `initialization_script` is only available on the builder API. **Test
  this independent of mpv entirely**, and note that running plain
  `cargo tauri dev` with no arguments will *always* fall back to the
  dev-preview default (`startTime: 12.0`, no real file) — that's expected,
  not a bug, since no `--init` was ever passed. To actually pass one
  through your normal dev workflow, `cargo tauri dev` forwards anything
  after a bare `--` to the compiled binary itself:
  ```
  cargo tauri dev -- --init '{"filePath":"C:/Users/you/Videos/ep1.mkv","fileName":"ep1.mkv","startTime":42.0,"subtitle":{"available":false}}'
  ```
  If the panel opens seeded at 42s with that filename shown, the handoff
  mechanism itself is confirmed working — what's left to debug at that
  point is the mpv Lua script specifically, not this Rust/JS side.
- ~~**Loading the actual video file**~~ — fixed: `app.js` now loads the
  video through Tauri's asset protocol (`convertFileSrc`) instead of a raw
  `file://` URL, and `tauri.conf.json` enables `security.assetProtocol`
  with a wide-open scope (`["**"]`) so any path mpv hands it can load.
  That open scope is fine for a personal tool driven only by your own mpv
  instance; narrow it to specific folders before sharing this with anyone
  else, since it currently lets the webview read any file on disk by path.
- ~~**Export failing with "No such file or directory"**~~ — fixed:
  `~/Videos/Clips` (the frontend's default output folder) was being
  passed to ffmpeg literally. `~` is a shell convention your terminal
  expands before a program ever sees it — ffmpeg has no idea what it
  means and tried to open a folder actually named `~`. `main.rs` now
  expands a leading `~` to the real home directory itself and creates the
  output folder if it doesn't exist yet, so this works whether or not the
  person has clicked Browse first.
- **Seek accuracy on phone-recorded video** — confirmed on a real file:
  MP4s with an "edit list" atom (common in phone/Google Photos exports —
  look for `handler_name: ISO Media file produced by Google Inc.` in
  ffmpeg's log) can print `Missing key frame while searching for
  timestamp` / `Cannot find an index entry` during export. Usually
  non-fatal — ffmpeg falls back to the nearest frame it can find — but
  worth double-checking the exported clip actually starts where you
  expect on this kind of source. If it's consistently off, switch the
  quality-mode MP4 path in `export_mp4` from input-side `-ss` (fast, can
  misbehave on edit-list files) to output-side `-ss` (slower, always
  accurate).
- ~~**Subtitle burn-in fails with "Unable to parse 'original_size'
  option value '0x0'"**~~ — fixed, confirmed against a real failing case
  (an MKV with embedded ASS subtitles + fonts). Root cause: ffmpeg's
  `subtitles` filter needs to know the source's real resolution to
  correctly scale ASS positioning, and its auto-detection of that failed
  in our filter chain — likely because `scale` ran *before* `subtitles`,
  leaving nothing for it to detect against by the time it ran. Two-part
  fix: `get_video_metadata` now also reports width/height (via ffprobe),
  which `subtitle_filter` passes through explicitly as `original_size=`
  rather than relying on auto-detection at all; and both `export_mp4` and
  `encode_gif_attempt` now apply `subtitles` *before* `scale` in the
  filter chain, which is also just the more correct order regardless (ASS
  coordinates are authored against the original resolution).
- **`mpv-scripts/clip-trigger.lua`** — reads the app's path from a
  `KLIPPIT_PATH` environment variable (falling back to a placeholder that
  triggers a clear on-screen warning if unset), rather than a hardcoded
  path baked into the script. This means moving from a `dev` build to an
  installed release build, or just moving the app at all, only ever needs
  that one environment variable updated — never an edit to the Lua file
  itself. Set it once in PowerShell:
  ```powershell
  setx KLIPPIT_PATH "C:\path\to\klippit.exe"
  ```
  (`setx` sets it permanently for future terminal sessions/processes,
  including ones mpv itself spawns — close and reopen any already-running
  terminal or mpv instance afterward for it to take effect.)
- ~~**Pressing the trigger key while a Klippit window is already open
  spawns a duplicate**~~ — fixed via `tauri-plugin-single-instance`.
  Registered first in `main.rs`'s plugin chain (required — it needs to be
  able to short-circuit everything else). When a second launch is
  detected, its `--init` argv is handed to a closure running inside the
  *already-running* process, which calls `window.applyInit(...)` (exposed
  globally in `app.js`, refactored out of what used to be one-shot
  top-level init code) directly on the existing window via `eval`, then
  focuses it — the second process exits without ever building its own
  window. **Not yet tested against a real double-trigger** — worth
  confirming pressing `c` twice in a row (on two different files, ideally)
  actually re-seeds the same window rather than doing something stranger.
- ~~**Screenshots routed through mpv's own commands instead of the
  app**~~ — reversed that decision. `extract_frame` in `main.rs` already
  existed but was never wired to a UI button; now it is (the small
  camera icon next to Play/pause), reusing the same Subtitles on/off
  toggle that governs export burn-in — "subs baked in or not" is the same
  question either way. The original reasoning ("mpv already has
  `screenshot video`/`screenshot subtitles`") was mpv-specific and broke
  down once VLC entered the picture, since VLC's own screenshot function
  doesn't offer that choice at all.

## Structure

```
src/                    panel UI — HTML/CSS/JS, opens standalone in a browser
src-tauri/              Tauri shell: Cargo.toml, tauri.conf.json, main.rs
mpv-scripts/            Lua trigger script for mpv's scripts directory
vlc-scripts/            Lua trigger scripts for VLC — see below
```

## VLC support

Klippit itself has no idea which player triggered it — the whole app
just consumes a generic `--init <json>` payload, whatever produced it.
Two VLC scripts were tried, in order:

- **`klippit-intf.lua`** (interface script, single-keypress attempt) —
  **tried and abandoned.** This aimed for the same one-key experience as
  mpv, by observing VLC's internal `key-pressed` libvlc variable. Tested
  against a real VLC install (standard videolan.org build) via both
  `--intf luaintf` and the corrected `--extraintf luaintf` invocations,
  with `--lua-intf klippit` pointing at the script — in both cases, no
  `[klippit] interface script loaded` message ever appeared in VLC's
  Messages window, meaning the script never loaded at all. Left in the
  repo for reference/future investigation, but not the path actually
  used.
- **`klippit-extension.lua`** (extension, menu-triggered) — **the one
  actually in use.** VLC's more standard, better-documented scripting
  surface — auto-discovered from a folder, no Preferences configuration
  needed. Trade-off versus mpv: triggered via View > Extensions > "Send
  to Klippit" rather than a single keypress. Still fully
  keyboard-reachable without a mouse (`Alt` → `V` for View → arrow to
  Extensions → `Enter`), just multi-step instead of one key. Install by
  copying to `%APPDATA%\vlc\lua\extensions\klippit.lua` (note:
  `extensions`, not `intf` — a sibling folder) and restarting VLC; no
  Preferences changes needed, it just shows up in the View menu.

Both scripts read `KLIPPIT_PATH` the same way `clip-trigger.lua` does for
mpv, and both share the same known gap: subtitle-track detection isn't
implemented for the VLC path yet — they always report "no subtitles,"
unlike the mpv script which properly detects the active track.

If menu-clicking turns out to be too much friction in practice, the
options discussed but not built are: an AutoHotkey global hotkey paired
with VLC's built-in HTTP remote-control interface (more moving parts,
but a real single keypress), or revisiting why `key-pressed` didn't fire
(could be a Lua syntax error VLC swallowed silently, or a genuine
behavior change in this VLC version versus whatever older examples of
this technique were written against).

## Layout: side-by-side editor, not a single vertical stack

Restructured from one long vertical column (video → scrubber → all
settings stacked below) into a proper editor layout: video + trim
controls in a growing left column (`#main-column`), export settings as a
fixed-width right sidebar (`#sidebar`, 280px). Window default/min size
changed from portrait (480×720) to widescreen (900×620 default,
700×480 minimum) to suit it.

This was a pure CSS/HTML restructure — every element kept its existing
`id`, just re-parented into new wrapping containers, so `app.js` needed
zero changes. Verified at both the new default and minimum window sizes,
including the tallest-content combination (Target-size mode + post-export
action buttons visible), with no scrolling needed at either size.

## Subtitle preview while editing (libass-wasm)

A plain browser `<video>` element cannot render embedded ASS/SSA
subtitle tracks from an MKV — this isn't a Klippit limitation, Chromium's
media pipeline simply doesn't support it. Fixed properly rather than
worked around: `src/lib/subtitles-octopus/` vendors
[libass-wasm](https://github.com/libass/JavascriptSubtitlesOctopus)
(libass — the same subtitle renderer mpv and VLC use internally —
compiled to WebAssembly), which overlays correctly-styled subtitles onto
the video element as a separate canvas layer, synced automatically to
its play/pause/seek state.

- **`extract_subtitles_for_preview`** (`main.rs`) pulls the first
  subtitle stream out of the source file (converted to ASS regardless of
  its original codec — SRT, SSA, whatever) plus any embedded font
  attachments, as standalone temp files, since libass-wasm needs these as
  separate inputs rather than reading them out of the video file itself.
  **The attachment-dumping ffmpeg command (multiple `-dump_attachment`
  flags batched into one call) is written from documented ffmpeg wiki
  patterns, not yet confirmed against a real run** — if font extraction
  comes back empty, that's the first place to check by running the
  equivalent command by hand.
- Shown whenever the source has subtitles, **independent of the export
  burn-in toggle** — seeing dialogue timing is useful for trimming
  regardless of whether you actually plan to burn subs into the final
  export.
- **Licensing note**: `libass-wasm`'s own wrapper code is MIT, but the
  compiled WebAssembly binary bundles libass, FreeType, HarfBuzz, and
  several fonts under a mix of licenses (LGPL, MIT, and a few others) —
  see `src/lib/subtitles-octopus/COPYRIGHT` for the full compound
  attribution, kept alongside the vendored files as required. This
  doesn't affect Klippit's own MIT license, but if you ever redistribute
  this project, that attribution file needs to stay with it.
- Only the WebAssembly worker was vendored, not
  `subtitles-octopus-worker-legacy.js` (a ~4.8MB non-WASM fallback for
  ancient browsers) — WebView2 fully supports WebAssembly, so that file
  would just be dead weight here.

## Post-export actions, progress, and a known video-preview quirk

- **Play → in-app review window, not the OS default app.** `open_review_window`
  builds a small second window (`review.html`) with a plain native
  `<video controls>` player, seeded via the same `initialization_script`
  mechanism as the main panel's mpv handoff. `capabilities/default.json`
  now lists both `"main"` and `"review"` under `windows` — a new window
  label needs to be added there or it won't have permission to call core
  APIs like `convertFileSrc`.
- **Editable output filename** — the Output section now has a filename
  field, defaulting to `{source}_{in}-{out}` but freely editable; once
  you type anything in it, it stops auto-updating as you move the in/out
  points (so your edit isn't silently clobbered). `export_clip` uses
  whatever's in that field as-is (sanitized for illegal Windows filename
  characters, and a stray `.mp4`/`.gif` you typed is stripped so the
  extension isn't doubled) rather than appending its own naming scheme on
  top of it.
- **Play/pause while editing** — a small ▶/⏸ button next to the seek bar,
  plus Space as a shortcut (guarded the same way Enter/Escape already
  are, so it doesn't hijack typing in text fields). Lets you actually
  watch the source while marking in/out, not just scrub statically.

- **"Show in folder" / "Play"** — added via `tauri-plugin-opener`, an
  official Tauri plugin purpose-built for exactly this (reveal a file in
  the OS file manager / open it with the system default app). Wiring is
  written against the documented v2 API
  (`window.__TAURI__.opener.revealItemInDir` /
  `.openPath`) but **not yet confirmed against a real build** — if these
  throw on first use, check that plugin's current docs for the exact
  method names, since plugin APIs shift slightly across point releases
  same as the shell/dialog ones did earlier.
- **Export spinner** — the spinning indicator on the Export button is
  **indeterminate** (just confirms something is happening), not real
  percentage progress. True progress needs ffmpeg's own `-progress
  pipe:1` output streamed live while it runs, which means switching
  `run_bin` from the current one-shot `.output()` call to `.spawn()` and
  listening for stdout events — a real architecture change, not a small
  addition. Worth doing once the core export path is confirmed reliable,
  not before.
- ~~**White screen on startup**~~ — resolved, and turned out not to be a
  bug at all: confirmed by testing against a second video file that the
  "white screen" was one specific source file (`Sentenced.mp4`) genuinely
  containing a white frame at the exact timestamp the panel happened to
  seek to. No rendering bug, nothing to fix. Removed the GPU-disable
  workaround and the CSS/JS mitigations that were written chasing this —
  they were solving a problem that never existed, and `--disable-gpu`
  specifically was pure downside (worse video performance) once that's
  clear.

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
4. Test the mpv↔app handoff without mpv first: run the built exe by hand
   with a real `--init` argument (see above) and confirm the panel opens
   seeded correctly. Only once that's confirmed, set the `KLIPPIT_PATH`
   environment variable to the built binary, drop the script in mpv's
   `scripts/` directory, and test the full end-to-end trigger.
5. Come back to single-instance messaging once the core loop works.
6. `cargo tauri build` once everything above works in `dev` mode — that's
   what actually produces the distributable `.msi`/`.exe`.
