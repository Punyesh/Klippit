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

## Subtitle overlay: three real bugs, found one at a time from actual
## evidence (not guessed)

Each of these was found from real console errors/screenshots, in order:

1. **`SubtitlePreviewData` missing `#[serde(rename_all = "camelCase")]`** —
   serialized as `ass_path`/`font_paths`, but `app.js` read
   `data.assPath`/`data.fontPaths` — both `undefined`, causing a `.map`
   crash on `undefined`. Fixed by adding the same attribute
   `VideoMetadata`/`ExportParams` already had.
2. **Backslash paths breaking `convertFileSrc()`** — `extract_subtitles_for_preview`
   returned Windows-native backslash paths; confirmed via a real
   `Loading data file "...%5CRoboto-Medium.ttf" failed` error containing
   a literal URL-encoded backslash. Fixed by normalizing to forward
   slashes, matching the same fix already applied to the export path's
   `subtitle_filter()`.
3. **Worker can't fetch Tauri's asset-protocol URLs at all** — even with
   a correctly-formed forward-slash URL, `subtitles-octopus-worker.js`
   (running in a Web Worker) still couldn't load the font file — a known
   category of issue with Worker-based libraries in Tauri/Electron-style
   apps, where the main thread can fetch custom-scheme URLs but a worker
   often can't. Fixed by fetching on the main thread (proven to work —
   same mechanism the video element itself uses) and handing the worker
   plain `blob:` URLs instead.
4. **Subtitle canvas positioned way outside the video area** — once text
   actually started rendering (confirmed real progress), it appeared
   stretched across the sidebar instead of confined to the video. Root
   cause: `subtitles-octopus.js` injects a new `<div
   class="libassjs-canvas-parent">` as a sibling of `<video>` to hold its
   canvas — inside `#preview-wrap`, a flex container with no explicit
   `flex-direction` (defaults to row), this became a second flex item
   competing with the video for space, breaking the library's own
   position math (which assumes a normal block-flow parent). Fixed with
   a CSS override taking that injected div out of flex flow entirely
   (`position: absolute; inset: 0`) so it stretches to fill
   `#preview-wrap` without disturbing the video's layout.

## Correct subtitle track selection (multiple tracks / external files)

Previously, extraction always grabbed whichever subtitle stream appeared
**first** in the video container — silently wrong if you were watching a
non-default track (a file with English + Spanish + French, say, where
you'd switched to Spanish), and completely blind to an externally-loaded
subtitle file (VLC's "Add Subtitle File," or mpv playing alongside a
same-named `.srt`) that isn't embedded in the container at all.

- **External file, when active, now takes priority** — `extract_subtitles_to`
  converts it directly rather than searching the container. mpv already
  exposes this via `current-tracks/sub/external-filename`, now actually
  wired through end to end (previously captured in the `--init` payload
  but silently discarded).
- **Embedded-track selection now matches by language**, not just "first
  found" — far more robust than trying to map a player's internal track
  numbering onto ffprobe's container stream indices, which aren't
  guaranteed to correspond. mpv reports a clean ISO code
  (`current-tracks/sub/lang`, e.g. `"eng"`); the Rust-side match tries an
  exact match first, falling back to a lenient substring check either
  direction. Falls back to the first stream found if there's no language
  match at all (same behavior as before this fix, not worse — just no
  longer the *only* behavior).
- **mpv side: high confidence.** The data was already being captured
  correctly; this just stops throwing it away. Multiple external
  subtitle files loaded simultaneously (e.g. several `.srt`s added at
  once) work correctly for free — mpv's `current-tracks/sub/*` properties
  always reflect whichever *one* is currently active, regardless of how
  many others are loaded alongside it.
- **VLC side: genuinely lower confidence** on the language-matching
  attempt — reads the active subtitle track's description via
  `vlc.var.get`/`get_list` on `"spu-es"`, wrapped in `pcall` so a wrong
  guess at VLC's API fails safely (falls back to "first stream," not a
  crash). What it reports is likely a human-readable name ("English")
  rather than a clean ISO code, which the lenient substring match is
  specifically there to give a real chance against.
- **VLC external subtitle files are not supported at all**, deliberately.
  VLC's Lua API doesn't appear to expose a full path for an
  externally-loaded subtitle directly (a known limitation) — only that
  same track description, which for an external file is typically just
  the bare filename ("subtitle.srt"). An earlier attempt guessed a full
  path by joining that filename with the video's own folder — dropped as
  too speculative to ship (a wrong guess would fail as a confusing ffmpeg
  error rather than a clear message). Instead, `klippit-extension.lua`
  sends `"trigger":"vlc"` in its payload, and `app.js` shows a plain,
  honest message when there's no embedded subtitle and the trigger came
  from VLC: *"No embedded sub detected. External sub handling not
  supported on VLC."* — rather than the generic message, which would
  read as if Klippit failed to notice something that's actually just
  unsupported.

## Subtitle timing fix + preview visibility toggle + screenshot control

Once export finally succeeded, a real, distinct bug surfaced: burned-in
subtitles were badly out of sync. Root cause: the export uses input-side
`-ss`, which rebases the video's own frame timestamps to start near zero
at the trim point — but the extracted subtitle file still carried its
*original absolute* timestamps (e.g. a line at 5:29 in the source), so
the subtitles filter was comparing "rebased-to-zero video time" against
"absolute subtitle time," wildly out of sync. Fixed by extracting the
subtitle stream with the **same trim window** as the actual export
(`extract_subtitles_to` now takes an optional `(in_time, duration)` and
applies matching `-ss`/`-t` during extraction), so both streams rebase to
the same zero-point consistently. Output-side seek is used for this
specific extraction (subtitle streams are cheap enough that frame-exact
accuracy costs nothing), while the main video encode still uses fast
input-side seek — so subtitle sync should be accurate to the same
tolerance the video's own trim point already is (the existing
keyframe-snap caveat documented elsewhere), not worse.

Also added, both straightforward: a **live preview visibility toggle**
(the "CC" button next to Play/Screenshot) that shows/hides the subtitle
overlay while editing — separate from the Subtitles Off/Burn-in toggle
in the sidebar, which governs the eventual export — and **screenshots
now respect this same preview toggle** rather than the export setting,
since "with or without subtitles" for a still is naturally answered by
whatever you're currently looking at on screen.

## Subtitle burn-in export: three attempts, the third one avoids the
## problem entirely instead of continuing to guess at ffmpeg's escaping rules

Each attempt was driven by a real, fresh error from an actual test file,
not guessed:

1. **`original_size=0x0`** — ffmpeg's auto-detection of this failed
   depending on filter order. Fixed by passing it explicitly. Necessary,
   but not the whole story.
2. **Bracket-heavy filenames corrupting the parser** — a real file like
   `[SubsPlease] Show - 09 [ABCD1234].mkv` broke the filter string when
   pointed at directly. Fixed by pointing at a cleanly-named extracted
   copy instead (`extract_subtitles_to`). Also necessary, also not the
   whole story.
3. **The Windows drive-letter colon itself** — even against a clean
   extracted path with no brackets, `No option name near
   '/Users/...':original_size=...'` kept recurring, in slightly different
   forms, across two different escaping strategies (single-quote-wrapping
   the value, then backslash-escaping the colon per ffmpeg's own
   documented rule for named options) — each time, the "near" text showed
   the parser choking right at the `C:` drive letter. Rather than keep
   guessing at the exact colon-escaping rule ffmpeg wants here, the fix
   sidesteps the problem entirely: `run_bin` now accepts an optional
   working directory, the subtitle-burn encode step runs with `cwd` set
   to the extracted subtitle's temp folder, and `subtitle_filter()`
   references it by **bare filename** (`subs.ass`) — no path, no drive
   letter, no colon, nothing left to escape at all. This threads through
   `export_mp4`, `run_ffmpeg_2pass`, `export_gif`, `encode_gif_attempt`,
   and `extract_frame` (screenshots), all of which now pass a matching
   `cwd` alongside the filter string wherever burn-in is active.

**Unverified**: `.current_dir(dir)` on `tauri-plugin-shell`'s sidecar
builder is written from its documented API (which mirrors
`std::process::Command`), not yet confirmed against a real compile — if
that exact method doesn't exist, the rest of this fix's logic (bare
filename + matching cwd) is still correct regardless of what that one
call ends up being.

`fontsdir` remains dropped (from the previous attempt) — burned-in
subtitles should display correctly, just potentially in a system
fallback font instead of the exact embedded one if it isn't already
installed. Revisit if font accuracy turns out to matter in practice.

## More real bugs found on closer re-inspection (not yet re-tested)

After the previous round of fixes was confirmed still failing on a fresh
rebuild, I re-traced the actual code (rather than guessing a third time)
and found genuine additional bugs:

- **Seek bar / playback still not moving the trim handle** — the real
  cause turned out to be different from the timing theory in the
  previous fix. `setHandleTime()` clamped `state.inTime` to never exceed
  `state.outTime` (and vice versa) — but `outTime` starts as just
  `startTime + 3` (three seconds after In). Since In is the default armed
  handle, and any real scrub/seek lands well past three seconds, In got
  silently pinned in place regardless of where you dragged — until Out
  was moved somewhere real first, which is exactly why clicking In/Out
  once appeared to "fix" it. Removed the cross-handle clamp entirely;
  `export_clip` already validates `in < out` at export time, so the UI
  doesn't need to enforce it live too, and doing so is what broke free
  scrubbing.
- **Subtitle burn-in export — a second bug beyond the filename-bracket
  fix.** `extract_subtitles_to`'s temp path comes from
  `std::env::temp_dir()`, which returns **backslash**-separated paths on
  Windows. The previously-working version of this filter always received
  **forward-slash** paths from the frontend (deliberately, from an
  earlier fix dodging JS backslash-escaping issues). `subtitle_filter()`
  now explicitly normalizes to forward slashes before embedding the path
  in the ffmpeg filter string, matching the format actually proven to
  work in this exact context rather than assuming backslashes behave
  identically once quoted.
- **DevTools were completely unavailable** — `Cargo.toml` never enabled
  the `devtools` Tauri feature, so right-click → Inspect likely didn't
  even work in the release build. Enabled now. This matters specifically
  for the subtitle-overlay-not-showing issue, which produces no visible
  error anywhere except the browser console (`[klippit] subtitle overlay
  error` / `subtitle preview extraction failed`) — **if subtitles still
  don't appear after this rebuild, opening DevTools and checking for
  those specific messages is the next real diagnostic step**, since nothing
  else in the codebase points to an obvious cause on inspection alone.
- **VLC extension silent failure** — `vlc.input.item()` can return `nil`
  for a brief moment right when VLC launches with a file, and clicking
  the extension in that window failed completely silently (a
  `vlc.msg.warn` line only visible in VLC's Messages window, which
  nobody has open). Now shows an actual dialog telling you to wait a
  moment and try again, rather than appearing to do nothing.

## ~~Subtitle burn-in fails on real-world filenames~~ — fixed properly

Confirmed on a real file: a name like `[SubsPlease] Show - 09 (720p)
[ABCD1234].mkv` — brackets and all — corrupted ffmpeg's filter-option
string parsing when the `subtitles` filter pointed directly at it,
producing a garbled `Unable to parse "original_size" option value` error
that had nothing to do with `original_size` itself. The earlier fix
(passing `original_size` explicitly) treated a symptom; this treats the
actual cause: `subtitle_filter()` and `extract_frame`'s burn-in path both
now point at a **cleanly-named extracted copy** of the subtitle track
(via the same `extract_subtitles_to` helper the preview overlay already
used) rather than the messy original filename, for both export and
screenshots. Absolute subtitle timestamps are preserved during
extraction, which is what keeps this correctly in sync with a trimmed,
`-ss`-shifted output — same timing assumption the original direct-file
approach relied on, just via an intermediate clean path instead of the
source file's own name.

## ~~Seek bar doesn't move the trim handle until you've touched
something else first~~ — fixed

The armed handle (In or Out) was only synced to `video.currentTime` via
the `timeupdate` event, which fires reliably during actual *playback*
but not necessarily for every rapid, discrete seek while paused (browsers
commonly throttle or skip `timeupdate` for that case) — so dragging the
plain seek bar could silently fail to move anything on the trim-track
ruler until some other action (clicking In/Out, which calls the sync
function directly) happened to establish it. Fixed by having the seek
bar's own drag handler call that sync function directly, rather than
depending solely on the event round-trip.

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

## ~~Bundled subtitles not detected~~ — fixed

Two different signals were getting conflated in `app.js`: mpv/VLC's
`subtitle.available` (whether a track is *currently selected in the
player* at trigger time — usually `false`, since VLC never reports it
and mpv only reports `true` if subtitles were actively on in mpv itself)
versus `get_video_metadata`'s `hasSubtitles` (a real `ffprobe` check of
whether the file *contains* a subtitle stream at all). `applyInit()`
disabled the Subtitles toggle based on the first, unreliable signal, and
`loadMetadata()`'s later, correct check never re-enabled it — so any file
with embedded subtitles got stuck showing "no subtitles." Fixed by
removing the premature disabling from `applyInit()` entirely and making
the `ffprobe`-based check in `loadMetadata()` the sole authority, in both
directions (disables *and* re-enables as appropriate).

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
