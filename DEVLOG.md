# Klippit — development log

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

## Review window couldn't play GIF exports

Real bug, confirmed via screenshot: reviewing a GIF export showed the
native "video failed to load" appearance (0:00 duration, gray
placeholder) instead of the actual content. Root cause: `review.html`
always used a `<video>` element regardless of file type, but GIF is an
image format (even when animated) — a `<video>` element's decoder simply
doesn't understand it, unlike MP4. Fixed by checking the file extension
and creating an `<img>` element instead for `.gif` paths, which handles
animated GIFs correctly and natively (including looping).

## v1.2.0: critical subtitle burn-in regression — root cause found

Reported as broken across the board: MP4 and GIF, cropped and
uncropped. That pattern (affects everything, no correlation with crop)
pointed away from anything in the crop work itself and toward something
shared by every export path — and subtitle burn-in specifically depends
on one thing nothing else does: `extract_subtitles_to`'s ffprobe
stream-probing step parses `run_bin`'s returned stdout line-by-line
(`.lines()`) to find which stream index has the subtitle track.

Root cause: the `run_bin` rewrite done for cancel_export (switching from
`.output()`, which returns the complete untouched byte stream in one
piece, to `.spawn()` with a streamed `CommandEvent` sequence) accumulated
`CommandEvent::Stdout`/`Stderr` chunks with plain `extend_from_slice`,
with no newline reinserted between them. Those events deliver data per
line with the trailing newline already stripped by the underlying
line-reader — so multi-chunk output collapsed into one run-on line with
no separators at all, silently breaking `.lines()`-based parsing
anywhere it was used. Fixed by pushing a `b'\n'` after each accumulated
chunk. Checked every other `run_bin` call site before landing this:
everything else either discards the returned stdout entirely (just
`?` for error-checking) or parses it as text (ffprobe JSON/CSV), where
an extra trailing newline is harmless — nothing anywhere treats this as
byte-exact binary data that the fix could corrupt.

Also worth answering directly, since it was asked: burn-in with crop
works exactly as designed, once this bug is fixed — crop runs first in
the filter chain, subtitles burn onto the already-cropped frame
(`original_size` is computed against the cropped dimensions
specifically, not the pre-crop source, so text is sized/positioned
relative to the frame it's actually drawn onto), and scale runs last.
This part of the implementation was never the problem here — the whole
pipeline was failing upstream, before crop or subtitles ever got a
chance to run.

## Three fixes from real testing feedback

**1. "failed to open preview: a webview with label `review` already
exists"** — a genuine race condition, not intermittent bad luck. Export
twice, press Play each time: `open_review_window` was calling
`.close()` on the existing review window, then immediately trying to
create a new one with the same label — but `.close()` doesn't wait for
the old window to actually finish tearing down before returning, so a
fast second call could hit Tauri's internal registry while the old
window was still technically registered. Fixed by reusing the existing
window instead of closing and recreating it: `review.html` now exposes
`window.applyReviewPath(path)` (mirrors the main panel's `applyInit`
pattern), and `open_review_window` calls that via `window.eval(...)`
when a review window is already open, re-centering and re-raising it,
rather than ever closing and recreating within the same session. This
sidesteps the race entirely instead of trying to work around it with a
wait/retry loop.

**2. Crop handles overlapping the window's own edge.** Real usability
issue: a full-frame crop's corner handles sit right at the video's own
displayed edge, which — with nothing else providing separation — can
coincide with the actual OS window edge (specifically the left side;
top/right/bottom already have the header/sidebar/frame-bar in the way).
Easy to grab the window's own resize handle by mistake instead of a
crop handle. Fixed with a 10px safety margin, active only while crop
mode is on (`#preview-wrap.crop-active`) — `getVideoDisplayRect()` was
updated to account for this same margin value when computing where the
video is actually displayed, so the crop overlay still lines up
correctly with the (now slightly smaller) visible video area rather
than drifting out of sync with it.

**3. Edge (width/height-only) handles added, 4 + 4 = 8 total.** The
existing corner-resize logic already computed an anchor (opposite
corner) and a "goes left/up" direction from the handle name's letters,
so this generalized naturally to single-letter edge names (n/s/e/w)
rather than needing separate logic: `hasHorizontal`/`hasVertical` flags
derived from the handle name decide which axis actually changes for a
free-aspect drag. The one genuinely new case is an edge handle with a
LOCKED aspect ratio — dragging just the right edge, say, with 16:9
selected, has no natural opposite-edge anchor for the vertical axis
that has to move to preserve the ratio, so that axis grows/shrinks
symmetrically around the box's own center instead. Corner drags with a
locked ratio are unaffected — still anchored at the opposite corner,
exactly as before.

Verified by actually executing `app.js` (not a static mockup) through
`wkhtmltoimage` again, extending the same approach from the crop
feature's original verification: all 8 handles render at their correct
positions, and the safety-margin gap between the video display area and
`#preview-wrap`'s own edge is visibly present once crop mode is active.

## Crop feature — designed, mocked up, built, and actually verified running

Static crop applied uniformly to the whole clip (the animated/keyframed
pan-and-follow version was explicitly scoped out as genuinely bigger,
better suited to a real video editor). Designed first as a real mockup
using Klippit's own CSS rendered through wkhtmltoimage, rather than a
generic wireframe — dimmed exterior showing what gets cut away, a bright
undimmed crop window with rule-of-thirds grid and corner handles, live
pixel-dimension readout, matching the app's existing "pair every visual
control with a numeric readout" pattern.

**Design decision made before building**: dropped a planned seek-row
toggle button in favor of a single sidebar Off/On control. Mute and CC
live in the seek-row because they're about the live preview experience;
crop is fundamentally an export setting (like Burn-in subtitles) that
happens to need an interactive overlay — two controls for the same state
would have been confusing, not extra convenience.

**Implementation**: crop coordinates live in state as SOURCE VIDEO PIXEL
values throughout, never screen pixels — converted to/from screen
coordinates only at render and drag time via `getVideoDisplayRect()`,
which computes the actual displayed video rectangle accounting for
`object-fit: contain` letterboxing/pillarboxing. This keeps the stored
crop correct regardless of window resizing or preview letterboxing
changes, and was flagged as a real implementation detail during the
design discussion before any code was written.

Backend: `crop_filter()` takes raw x/y/w/h values (not `&ExportParams`
directly) specifically so it's reusable from `extract_frame`
(screenshots) too — a screenshot should match whatever crop the live
preview is currently showing, same "screenshot shows what you're
currently seeing" principle already established for subtitles. Crop
runs first in the filter chain, before subtitles and scale — subtitle
`original_size` is computed against the CROPPED dimensions when crop is
active (not the original source), since subtitles burn onto the
already-cropped frame and need to be sized/positioned relative to that.

**Verification — a real testing-tool discovery along the way**: initial
attempts to visually verify the actual (not mocked) implementation
showed `app.js` apparently never executing at all inside
`wkhtmltoimage` — every top-level variable (`video`, `previewWrap`, etc.)
came back `undefined` with no error. Root cause: `wkhtmltoimage` blocks a
local HTML file from loading other local files (like an external
`<script src="app.js">`) unless `--enable-local-file-access` is passed —
a security default in that tool, unrelated to anything in the app code.
Every prior visual check in this project had sidestepped this entirely
by stripping `<script src="app.js">` out before rendering (testing
static HTML/CSS only) — this was the first time actually executing the
full app logic through this tool, so the limitation had never surfaced
before. With that flag, `app.js` runs correctly and every crop function
is properly defined.

With execution confirmed working: toggling crop on, applying a 9:16
preset against a simulated 1280×720 source correctly produced
`cropX=438 cropWidth=405 cropHeight=720` — exactly right (405/720 =
9/16) — with the overlay rendering visibly, correctly positioned and
sized, sidebar readout matching the on-video label. The core, most
novel logic (state management, letterboxing-aware positioning math,
aspect-ratio calculations) is genuinely confirmed working, not just
assumed.

**What's still unverified**: the actual mouse-driven drag interactions
(move the box, resize from a corner). Attempting to simulate these via
synthetic `PointerEvent` construction failed silently — `wkhtmltoimage`'s
underlying engine (a pre-2016-era WebKit build) doesn't support the
`PointerEvent` constructor at all, which is the exact API these
listeners use (`addEventListener('pointerdown', ...)`). This is a
limitation of the decade-old test engine, not the app — the real
Klippit runs in a modern, fully Pointer-Events-compliant WebView2
context — and the drag code itself follows the same well-established
pointerdown-on-element / pointermove-and-pointerup-on-document pattern
already proven working for this app's original trim handles earlier in
the project. But this specific piece — does dragging the box actually
feel right, does corner-resize with a locked aspect ratio behave as
expected — needs real mouse input in the actual running app to fully
confirm.

## Volume slider — the mute toggle needed one

Fair, immediate follow-up to the mute/unmute toggle: turning audio on
with no way to set the level isn't much of a feature. Added a compact
volume slider next to the mute button, shown only while unmuted (kept
the seek-row uncluttered in the default muted state rather than showing
a volume control that does nothing until you've opted into audio at
all). Confirmed visually before shipping — sits naturally in the
existing button row, matches the seek-bar's own thumb styling.

## Subtitle load delay + preview audio toggle

Two more real questions from user feedback:

**"Subtitles sometimes don't show up for a bit."** Real and explainable,
not random: loading the live-preview overlay involves probing for the
subtitle stream, extracting the `.ass` file and fonts, fetching each as
a blob, then loading and initializing a ~2.7MB WASM library — all
happening *after* the video itself is already visible. Two
improvements, neither eliminates the delay entirely but both help:

- `loadSubtitleOverlay()` now kicks off speculatively in `applyInit()`,
  in parallel with `loadMetadata()`, rather than waiting for metadata's
  `hasSubtitles` confirmation first — extraction and metadata are
  independent ffprobe/ffmpeg calls on the same file, so there was no
  reason to run them sequentially. For a file with no subtitles this
  just fails harmlessly (already handled); for one that does have them,
  this head start measurably cuts the real delay.
- The CC button now shows a visible pulsing "loading" state
  (`setSubtitleLoadingIndicator`) while the overlay is being prepared,
  cleared at every exit path (ready, error, or extraction failure) — the
  wait is now visible and expected rather than reading as "subtitles
  just don't show up."

**"Is audio playback implementable?"** Yes, trivially, technically — the
`<video>` element is just hardcoded `muted`. The real question was a
design one: Klippit is triggered from inside mpv/VLC, which is often
still open with the same file, so audio on by default risks an echo if
the original player isn't paused. Added an opt-in mute/unmute toggle
(speaker icon, next to Play) — muted by default matching the existing
attribute, one click to hear audio for anyone who has paused or closed
the original player.

## v1.1.0: cancel export, fixed always-on-top behavior, GIF bug investigation

Four requests handled together:

**Cancel export button.** Genuinely new API surface for this project:
every ffmpeg/ffprobe call so far used `tauri_plugin_shell`'s `.output()`,
which spawns and awaits a process as one atomic operation with no handle
you can act on mid-flight. Switched `run_bin()` to `.spawn()` instead,
which returns a `CommandChild` you can `.kill()` and an event stream
(`CommandEvent::Stdout/Stderr/Terminated/Error`) you read from as the
process runs — the same total information `.output()` gave, just
obtained incrementally instead of all at once. A new `ExportState`
(managed Tauri state: a `Mutex<Option<CommandChild>>` plus a cancelled
flag) holds whichever child process is currently running; a `cancel_export`
command locks it and kills it. A single slot is correct here, not a
limitation: one export runs its ffmpeg steps strictly sequentially, so
killing "whichever one is currently running" when cancel is pressed is
exactly right, and the resulting error aborts the rest of the sequence
through the same `?` propagation already used everywhere — no separate
cancellation plumbing needed through `export_gif`/`export_mp4`/etc.
`run_bin`'s signature and return type are unchanged, so no other call
site in the file needed to change. Frontend: the existing "Escape"
button now doubles as "Cancel" while an export is running (closing the
whole panel mid-export isn't something you'd want anyway, so repurposing
that slot fits naturally), and a cancelled export shows a clear "export
cancelled" status rather than a generic failure message.

**Always-on-top wasn't behaving correctly.** Real, valid complaint:
`always_on_top(true)` set at window-creation time kept both the main
window and the review window permanently pinned above every other
application for as long as they stayed open — switching to a browser or
file explorer while Klippit was open meant it kept forcing itself back
in front. The main window and the single-instance re-focus path had
already been fixed earlier in this session (toggling
`set_always_on_top(true)` then immediately `false` — a legitimate Win32
pattern: both calls are synchronous and immediately update window
z-order, so this reliably raises the window once without leaving the
persistent pinned flag set). The review window still had the old
permanent version on its builder — fixed to match the same toggle
pattern.

**GIF ignoring the Out marker — investigated, not conclusively fixed.**
Reported by an end user, not reproducible here. Retraced the entire
path (frontend `state.outTime` capture, `export_clip`'s duration
computation, the extraction step's `-t` placement) and found nothing
incorrect — the extraction step has exactly one input, so `-t` there is
unambiguous the same way MP4's always was, unlike the two-input
paletteuse pass that caused the earlier, now-fixed duration bug. Given
no reproduction and no bug found on review, added diagnostic logging of
the exact received `in_time`/`out_time`/`duration`/format/mode to
`%TEMP%\klippit-ffmpeg.log` at the very start of `export_clip` — if this
recurs, the log will show definitively whether wrong values were ever
received (a frontend issue) versus received correctly but mishandled
further down the pipeline (a backend issue), a distinction currently
impossible to tell from the symptom alone. Most likely explanations
without further evidence: the reporting person was on an older build
predating the `-t` fix (the same "did you actually reinstall" confusion
hit repeatedly earlier in this project), or Out was left at its
just-3-seconds-past-In default and happened to reach the file's actual
end for a short clip near the end of a video.

**Version bumped to 1.1.0** — genuinely new features (cancel export,
GPU encoding, the Settings panel) accumulated since 1.0.0, not just bug
fixes, warranting a minor version bump rather than a patch.

## Klippit's first Settings panel — configurable mpv keybind + portable_config support

Driven by real user feedback from actual community testing:

1. Someone's mpv used `portable_config` (a folder next to `mpv.exe`
   itself, which mpv prefers over `%APPDATA%\mpv` when present) — the
   automatic setup always wrote to `%APPDATA%` unconditionally, so the
   script silently landed somewhere their mpv never looked, with no
   error or indication anything was wrong. They had to find this
   themselves and manually copy + edit the script into portable_config.
2. Separate, sharper feedback: "shortcut edit should be in the app
   itself" — and the pointed follow-up, "if people are having to edit
   lua scripts they might as well run ffmpeg on command line." Exactly
   right: requiring a script edit to change a keybind undermines the
   entire point of the app being easier than hand-rolled ffmpeg.

Fixed both together, since they share the same underlying need — a real
settings surface:

- **`clip-trigger.lua` no longer hardcodes its keybind.** It reads one
  via mpv's own `mp.options`/`script-opts` mechanism
  (`script-opts/clip-trigger.conf`, a single `key=X` line), defaulting
  to `c` if that file doesn't exist. `read_options` respects mpv's own
  portable_config resolution automatically — the script itself doesn't
  need to know or care which mode mpv is in.
- **Klippit's first-ever persisted settings**: a plain JSON file at
  `%APPDATA%\Klippit\settings.json` (`KlippitSettings`: `mpv_keybind`,
  `mpv_config_dir_override`) — chose a plain file over the registry for
  simplicity (no new dependency, std::fs alone) and so it's easy for
  someone to inspect or delete by hand if something goes wrong.
- **`run_setup()` now resolves the mpv config directory from these
  settings** (the override if set, else the previous hardcoded
  `%APPDATA%\mpv` default) and additionally writes the keybind config
  file alongside the script — both the one-shot installer `--setup` call
  and the background self-healing check on every normal launch now keep
  the keybind in sync automatically, the same way the script itself
  already was.
- **A real Settings panel in the app** (new gear icon in the header,
  first modal dialog this app has needed) with two fields — mpv keybind,
  mpv config folder override — and a "Save & Reinstall" button that
  saves the settings and immediately reruns the same `run_setup()`
  machinery, showing a plain-text report of what happened. This is the
  concrete answer to "should be in the app itself": changing the keybind
  is now a text field and a button, never a file edit.

`#head`'s CSS needed a small adjustment: it assumed exactly two children
under `justify-content: space-between`, which breaks once a third
(the settings button) is added — switched to `gap` plus giving
`.source-name` `flex: 1 1 auto; text-align: right` so it fills the
middle regardless of child count.

Confirmed via rendering (again): `inset: 0` on the modal's
`position: fixed` overlay rendered completely wrong in the quick
visual-check tool used throughout this project — same shorthand-support
gap in that old renderer hit twice before. Switched to the longhand
(`top/right/bottom/left: 0`) for both correctness there and consistency
with the earlier fixes.

## Opt-in GPU encoding, with automatic fallback

A follow-up to the earlier GPU-encoding discussion: not a replacement of
the software encoder, an explicit "CPU / Try GPU" toggle (same segmented
pattern as Audio/Subtitles), scoped to MP4 Quality mode only — GIF is
palette-based with no H.264 encoder involved at all, and Target-size
mode's 2-pass encoding has far less consistent hardware support across
encoders/drivers, so both stay CPU-only for now.

When GPU is ticked, `encode_quality_mode` tries `h264_nvenc`, then
`h264_amf`, then `h264_qsv`, each with a roughly CRF-equivalent quality
option for that encoder. Nothing here detects which GPU vendor is
actually present — it just attempts each in turn and moves on
immediately if ffmpeg reports that one can't be used, which naturally
handles "wrong or no GPU vendor" without needing to know that in
advance. If every hardware option fails, falls straight through to the
exact same `libx264` path used when GPU isn't requested at all. This
can't produce a broken export: the worst case for a wrong quality-
parameter guess on some encoder is that one attempt fails and the loop
moves on, exactly as if that GPU didn't exist.

`export_clip`'s return type changed from a plain path string to a small
`ExportResult { output_path, encoder_used }` struct, so the frontend can
say plainly whether GPU encoding actually happened or silently fell back
to CPU — visible in the status message, not just the diagnostic log.

Genuinely unverified: the exact quality-parameter syntax for each
hardware encoder (`-cq` for nvenc, `-qp_i`/`-qp_p` for amf,
`-global_quality` for qsv) is written from each encoder's documented
options, not confirmed against a real GPU of each vendor — there's no
way to test that here.

## Startup performance pass

Two low-risk fixes, both about deferring work rather than changing what
anything does:

- **`subtitles-octopus.js` was loading eagerly** on every single launch
  via a `<script>` tag in `index.html`, regardless of whether a file was
  even loaded yet or had subtitles at all — true for most launches.
  Removed that tag; the library now loads dynamically on first actual
  use (`ensureSubtitlesOctopusLoaded()`, cached after the first load so
  switching between subtitled files doesn't reload it repeatedly).
- **Drag-drop setup deferred via `setTimeout(fn, 0)`** rather than
  running immediately at script load — nothing about the initial render
  depends on it, so there's no reason for it to compete with getting
  pixels on screen first.

Also parallelized subtitle extraction: `extract_subtitles_to`'s ASS
extraction and font extraction are independent operations (neither reads
the other's output, both just read the same source file and write to
different destinations) that were running sequentially. Split font
extraction into its own function and spawned it via
`tauri::async_runtime::spawn` to run concurrently with ASS extraction
instead of waiting for it to finish first — same proven spawn mechanism
already used for the background setup check, low risk since there's no
shared mutable state between the two.

Noted but not changed: "quite slow on first startup" specifically is
very likely WebView2's own one-time runtime initialization (creating its
user data folder, JIT-warming, etc.) — a well-known cost for any
Tauri/Electron-style app's first launch after install, and not something
under this app's own control.

On GPU-accelerated encoding and multithreading, asked about together:
multithreading is already happening — `libx264` (the current encoder)
auto-detects and uses available CPU cores by default; nothing here was
ever limiting it to one thread. GPU encoding (NVENC/AMF/QSV — the
bundled ffmpeg build does have these compiled in) is a genuinely
different proposition than the fixes above: not every machine has a
compatible GPU, so switching encoders without real fallback logic would
break exports outright for anyone without one, and hardware encoders
have different quality-per-bitrate characteristics that could affect
target-size mode's accuracy in ways that need actual testing to trust.
That's a real, separate feature — proper try-hardware-then-fall-back
logic — not a "broad, low-risk" change, and given every actual
performance bug found and fixed in this project so far has been about
unnecessary seeking rather than raw encode throughput, it's also not
obviously the right lever for this app's typical short-clip workload
specifically. Deferred rather than implemented blind.

## GIF export still slow after the duration fix — real, separate cause

The `-t` argument-position fix was real and necessary (confirmed: output
size went from 579MB to a correct, proportionate 0.98MB), but a genuinely
separate problem remained: the same ~4-second export still took ~6
minutes. Checked the diagnostic log for the exact commands running,
which showed the seek position: 548 seconds — about 9 minutes into the
source file.

Root cause: both GIF passes (`palettegen` and `paletteuse`) independently
seek into the original source file at that same deep position. Whatever
that seek/decode cost is for this file, it gets paid **twice** per
export, and in target-size mode, up to **six times** across retry
attempts — quite possibly explaining minutes of overhead for what should
be a few seconds of actual encoding work.

Fixed by restructuring `export_gif` to extract the target window into a
small intermediate file **once**, up front, then running both GIF passes
(and every target-size retry) against that already-trimmed file, which
starts at position 0 — no seeking needed there at all, regardless of how
many passes follow. `encode_gif_attempt`'s signature simplified
significantly as a result: it no longer needs `params`, `duration`,
`subtitle_filter_str`, or `subtitle_cwd` at all, since seeking and
subtitle burn-in both now happen exactly once, in the upfront extraction
step, rather than being repeated in every pass.

One important correctness detail caught before shipping: the upfront
extraction is always re-encoded, deliberately never stream-copied even
when there's no subtitle filter to apply. Stream copy (`-c copy`) can
only cut at keyframes, since it never decodes anything — meaning the
extracted segment could start several seconds before the actual
requested in-point if the nearest keyframe is far away, silently
shifting the whole clip earlier than intended. Re-encoding this tiny
few-second segment costs well under a second on any modern machine,
negligible next to the seek-avoidance this whole change is for — and
frame accuracy is the actual point of this app, so that wasn't a trade
worth making to save a fraction of a second.

## GIF export: confirmed root cause and fix — -t argument position bug

The diagnostic logging paid off immediately, along with a second real
data point: the export DID eventually finish, producing a 579MB GIF for
a 3-second selection (222KB for the identical selection as MP4). That
size difference confirmed the leading theory precisely.

Root cause: `encode_gif_attempt`'s second ffmpeg call (the actual
paletteuse encoding pass) has TWO inputs — the source video and the
generated palette image. `-t duration` was placed between the two `-i`
flags:

```
-ss <in> -i <video> -t <duration> -i <palette> -lavfi "..." <out>
```

With two inputs, an option placed between `-i` flags gets read as an
INPUT option for whichever input follows it — so `-t duration` was being
applied to the palette image input (meaningless for a static image)
instead of capping the actual output duration. The video input was left
completely uncapped, decoding from the seek point all the way to the end
of the file. This is the only one of this project's ffmpeg commands with
two inputs, which is exactly why MP4 export (always exactly one `-i`,
so no such ambiguity exists) was never affected, while this specific GIF
pass always was.

Fixed by moving `-t duration` to after BOTH `-i` flags, immediately
before `-lavfi`, where it's unambiguously an output-level option
regardless of how many inputs precede it. The first GIF pass (palette
generation) never had this bug — it only ever has one input, so `-t`
there was always unambiguous.

## GIF export hanging — diagnostic logging added, root cause still open

Real report: GIF export for a 3-second clip never completing, confirmed
still running via Task Manager at 16% CPU — not a hard deadlock (that
would show 0%), but far below what a genuine few-second encode should
need. MP4 export for a similar clip is fast, which rules out the shared
seek/input mechanism and narrows this specifically to the GIF pipeline's
own two-pass palettegen/paletteuse commands.

Couldn't pin down the exact cause through code review alone — added
`log_command()`, called from `run_bin()` for every single ffmpeg/ffprobe
invocation, appending the exact command line (plus cwd, if set) to
`%TEMP%\klippit-ffmpeg.log`. Best-effort, never affects the actual
export if logging itself fails. Leading theory going in: a seek or
duration argument not correctly limiting one of the two GIF-pipeline
ffmpeg passes to the intended short window, causing it to decode much
further back in the file than intended — this log will confirm or rule
that out directly against the real command, rather than continuing to
reason about it from the source alone.

## Review window: wrong position, controls that appeared missing

Real bug, confirmed via a real screenshot after an export: the review
window ("Klippit — Preview," opened by the Play button) had a fixed size
but no explicit position set at all — it could spawn anywhere, including
overlapping confusingly with the main window, or ending up behind other
applications entirely (VLC, in this case). Read as "the video preview
appears below the main screen, not on top."

`.center()` handles the position. For actually appearing above
everything — not just the main window but other applications too —
`.set_focus()` alone isn't a strong enough guarantee: Windows has its
own focus-stealing-prevention rules that can block a window from
grabbing foreground status depending on timing and which process is
asking. `always_on_top(true)` is the real fix, the same mechanism
already used for the main window itself and for the same underlying
reason; `.set_focus()` stays in as a secondary nudge, not the primary
mechanism.

The "no player controls" symptom turned out to be a red herring — tried
removing `autoplay` on the theory that native controls fade out during
playback without mouse hover, but that theory wasn't the actual
complaint; reverted, `autoplay` stays.

## Manual file loading when opened without mpv/VLC context

Following directly from the dev-preview label fix above: if there's no
file loaded, there should be an actual way to load one, not just an
accurate label saying there isn't one. Added a drop-zone overlay inside
the (otherwise empty, black) preview area, shown only when
`!init.filePath`:

- **Browse button** — high confidence, reuses the exact same
  `tauri-plugin-dialog` `open()` call already proven working for the
  output-folder picker, just configured for file selection with video
  extension filters instead of directory selection.
- **Drag-and-drop** — lower confidence, new API surface
  (`getCurrentWebviewWindow().onDragDropEvent`) not exercised elsewhere
  in this project. Wrapped in try/catch so a wrong guess at the exact
  method/payload shape fails silently to console rather than breaking
  anything — the Browse button remains a fully reliable way in
  regardless of whether this works exactly as coded.

Both paths funnel into the same `applyInit()` used by every other
trigger source (mpv, VLC, single-instance re-seed), building the same
shaped object rather than duplicating any of that reset logic.

Confirmed via rendering that `inset: 0` (the CSS shorthand) rendered
completely wrong in the quick visual-check tool used throughout this
project (`wkhtmltoimage`, a very old WebKit-based renderer that likely
doesn't support that property) — switched to the equivalent longhand
(`top/right/bottom/left: 0`) for both correctness in that tool and extra
certainty in the actual Chromium-based webview, which should have
supported the shorthand fine regardless.

## "(no file — dev preview mode)" shown in a real, working install

Confirmed real confusion: launching `klippit.exe` directly (not via
mpv/VLC) showed the exact same "dev preview" wording used for the
genuinely different case of opening this in a plain browser tab with no
Tauri backend at all. The app was working correctly — just no file
context yet, since no `--init` argument was passed — but the wording
made it look broken. Split into two accurate messages based on whether
`window.__TAURI__` is actually present: "no file loaded — trigger from
mpv or VLC" for a real backend with nothing to edit yet, "dev preview,
no Tauri backend" reserved for the actual browser-only case. Both still
start with `(no file`, preserving the existing prefix-check used
elsewhere to detect "no real file loaded."

## Automatic setup: env var, mpv script, VLC extension — no manual steps

**Real failure confirmed on first actual install**: `KLIPPIT_PATH`
failed to set while both scripts succeeded. Root cause: `setx` is a
console application, and since `klippit.exe` is a GUI app with no
console of its own, Windows opens a new visible console window for
`setx` to run in — closing that window before `setx` finishes writing
to the registry kills it mid-operation. Fixed with the `CREATE_NO_WINDOW`
process creation flag on Windows, so `setx` now runs with no window at
all — nothing left for anyone to accidentally close. The background
self-healing check (see below) means this corrects itself automatically
on the very next launch once rebuilt, no reinstall required.

This whole session, every "did you re-copy the updated script" and
"remember to set KLIPPIT_PATH" moment was manual friction. Replaced with
an approach split across confidence levels, so the parts I'm sure about
work regardless of whether the one uncertain part does:

**High confidence — `run_setup()` in `main.rs`.** Pure Rust/std,
no exotic APIs: sets `KLIPPIT_PATH` via `setx` (the exact mechanism
already manually verified working earlier this session), and copies the
bundled `clip-trigger.lua` / `klippit-extension.lua` into `%APPDATA%\mpv\
scripts\` and `%APPDATA%\vlc\lua\extensions\`, creating those folders if
needed. Always overwrites rather than checking first — deliberately, so
a script updated in a newer Klippit build automatically reaches the
person's mpv/VLC config on their very next launch, rather than needing
another manual re-copy.

This runs two ways:
- **Once, headlessly**, via `klippit.exe --setup` — no window, no Tauri
  Builder, just the setup logic and a log file, then exit. This is what
  the installer calls (see below).
- **In the background, every normal launch**, spawned right after the
  window is built (so it adds no perceptible startup delay — the window
  appears immediately either way) as a self-healing safety net. Covers
  both "the installer's assumed resource-folder layout wasn't quite
  right for this build" and "the person moved or reinstalled Klippit
  since the last install."

A log always lands at `%TEMP%\klippit-setup.log` regardless of which
path ran, and if the background check finds a real failure, a
dismissible banner appears in-app (`window.__klippitSetupWarning`) —
a working setup stays invisible, a broken one doesn't fail silently.

**Moderate confidence** — `tauri::async_runtime::spawn` for the
background task, and capturing the window handle into it. A standard
Tauri pattern for exactly this "something after setup, without blocking
it" case, but a less-tested pattern in this codebase than the
async-command-driven code that makes up most of it.

**Unverified — `installer-hooks.nsh` and its `tauri.conf.json` wiring**
(`bundle.windows.nsis.installerHooks`). This is genuinely just one line
(`ExecWait '"$INSTDIR\klippit.exe" --setup'`) inside a
`NSIS_HOOK_POSTINSTALL` macro — the actual setup logic lives entirely in
the high-confidence Rust code above, this file's only job is triggering
it during the install wizard instead of a moment after. If `cargo tauri
build` fails specifically on this file or that config key, that's the
exact thing to report back — the background self-healing check still
covers everything correctly regardless, just a moment later than ideal
rather than not at all.

Also added: `bundle.resources` in `tauri.conf.json`, mapping both
scripts into the installer output so they're available at runtime for
`run_setup()` to find and copy.

## Header: dropped the redundant "KLIPPIT" text

The OS window title bar already shows the app name and icon; the app's
own in-window header repeated it as text right below, which looked
doubled in any view showing both together (e.g. a taskbar hover
preview). Replaced the text brand with a small inline SVG version of the
same K-mark used for the app icon — a quiet visual anchor instead of a
redundant label.

## Trim bar is now purely visual — dragging and auto-pause both removed

Real, reported friction: playback would refuse to advance past an old
Out marker even while just navigating toward a new mark, because of an
"auto-pause at your Out point" convenience added during the Mark
In/Mark Out redesign. That convenience directly contradicted the
redesign's own premise — navigation is supposed to be completely free —
so it's removed entirely, along with handle dragging (which was already
downgraded to "supplementary" in that same redesign, and turned out not
to be worth keeping once Mark In/Mark Out covered the actual job).

The ruler and its two handles now do exactly one thing: show where In
and Out currently are. `#handle-in`/`#handle-out` changed from
`<button>` to plain `<div>` (no focus, no interaction at all), CSS
cursor changed from `ew-resize` to `default`, and `pointer-events: none`
keeps them fully out of the way of anything else on the page. Removed
dead code this left behind: `makeDraggable()`, `stepHandle()`, and the
now-unused `trimTrack` DOM reference.

## Mute audio option + larger screenshot icon

Added an Audio Keep/Mute segmented toggle (same visual pattern as
Subtitles), hidden entirely for GIF output since GIFs never carry audio
at all. Quality mode uses `-an` instead of encoding an AAC track when
muted; target-size mode also skips reserving the usual 128kbps audio
slice in that case, giving video the full bitrate budget instead of
wasting part of it on a track that won't exist.

Screenshot icon bumped from 12px to 16px (with a slightly lighter stroke
width to match) — the camera shape wasn't reading clearly at the
original size, confirmed by rendering both side by side.

## Replaced the trim bar's interaction model: explicit Mark In/Mark Out

The old model had whichever point ("In" or "Out") was currently "armed"
continuously reassign itself to follow the playhead for any reason it
changed — playback, scrubbing, frame-stepping. That coupling, not the
handle-dragging itself, was the real source of the bar feeling finicky:
scrub around to find your out point and you could easily be silently
moving In instead without realizing which one was armed.

Replaced with the Sakuga Enhancer pattern, fully decoupled:

- **Playback, the seek bar, and `,`/`.` frame-stepping are pure
  navigation** — they move the video's playhead and nothing else. No
  side effects on In/Out at all anymore, including frame-stepping
  itself: an initial version of this redesign still had `,`/`.` nudge
  whichever point was armed directly, which didn't fit the new model —
  since marking now captures wherever navigation lands you, the stepping
  controls belong to the player, not to a marked point.
- **Mark In / Mark Out** (buttons, or **I** / **O** on the keyboard) are
  the only things that touch In/Out during normal use — explicit,
  stateless, one-click actions. Click one and wherever the playhead is
  *right now* becomes that point, immediately and completely. Nothing
  lingers afterward to show "which one is active," since there's no
  ongoing state left to track — a deliberate simplification from an
  earlier version that kept a persistent "armed" indicator on these
  buttons, which didn't fit "one click" either.
- **The natural flow**: scrub or frame-step to the exact spot, click
  Mark In, scrub/step to the other spot, click Mark Out. Frame-accurate
  by construction, since you're navigating to the exact frame before
  marking it, rather than nudging an already-placed point afterward.

Dragging the ruler handles directly still works as a supplementary,
deliberate method for direct handle manipulation (arrow keys nudge the
specific focused handle, distinct from general frame-stepping), just no
longer the primary way to set points, and no longer entangled with
playback. The one remaining automatic behavior: playback still
auto-pauses if it reaches your marked Out point, a simple "preview stops
at your out marker" convenience, independent of everything else.

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
