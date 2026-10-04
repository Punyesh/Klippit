# Klippit

A lightweight Windows video clip, GIF, APNG, screenshot, and screen-recording
tool. Trigger it from **mpv** or **VLC**, open a file directly, or record a
resizable region of your screen and immediately trim/edit the result in
Klippit.

![platform](https://img.shields.io/badge/platform-Windows-blue)
![license](https://img.shields.io/badge/license-MIT-green)

## Features

- **Trigger from mpv or VLC** — press `c` in mpv, or use the "Klippit"
  extension in VLC's View menu, right at the moment you want to start
  clipping.
- **Native screen recording workflow** — select a resizable region on one
  monitor, record with a hardware encoder where available, pause/continue,
  stop or discard, then the temporary recording opens directly in Klippit for
  trimming, cropping, speed changes, screenshots, and export.
- **Recorder controls for FPS, codec, quality, cursor, system audio, and
  microphone**, with global `F9` Start/Pause/Continue, `F10` Stop, and `F11`
  Discard shortcuts while the recorder is open.
- **Disk-backed recorder workflow** — native segments are written to temporary
  files rather than held in RAM. Klippit prompts before discarding an unsaved
  finalized original and can offer finalized recordings/segments on the next
  launch.
- **Explicit Mark In / Mark Out** — scrub or frame-step to the exact
  frame, then mark it. No dragging tiny handles, no guessing.
- **MP4, GIF, or APNG export**, with Quality and Target-size workflows. MP4
  uses CRF / bitrate-based encoding; GIF and APNG use their resolution/FPS
  controls and automatically reduce them when needed for a size target.
- **Subtitle burn-in**, matched correctly against multi-track files (by
  language) and external subtitle files (mpv only), with a live preview
  overlay you can toggle on/off while editing.
- **Screenshots**, with or without subtitles, matching whatever the live
  preview is currently showing.
- **Multi-section clips** — combine several source ranges into one export,
  with independent speed and crop/framing for each section.
- **Per-section crop/zoom** — keep one project-wide aspect ratio while each
  section can pan and zoom independently; Klippit normalizes the sections
  automatically before joining them.
- **Mute audio** option for MP4 exports.
- Frame-accurate stepping (`,` / `.`, or the on-screen buttons), a full
  keyboard-driven workflow (`I` / `O` to mark, `Space` to play/pause,
  `Enter` to export).

## Installing

Download the latest installer from the
[Releases page](../../releases) and run it — that's it. No manual setup,
no environment variables to configure, no scripts to copy anywhere.
Klippit configures its own integration with mpv and/or VLC automatically
the first time it runs.

**Requires:** Windows. [mpv](https://mpv.io) or
[VLC](https://www.videolan.org/vlc/) is optional and only needed for the
media-player trigger integration. Klippit can open files and record the screen
without either player installed.

## Using it

**From mpv:** while a video is playing, press `c` at the moment you want
your clip to start. Klippit opens, seeded to that exact timestamp.

**From VLC:** open a video, then **View → Extensions → Klippit**.

**Or open Klippit directly** (Start Menu, desktop shortcut) and drop a
video file onto it, or click **Browse for a file…**.

**To screen-record:** click the red recorder icon at the left of Klippit's main
toolbar. Move and resize the capture frame to the region you want, then press
**Start** (or `F9`). The frame turns red and locks while recording. Recorder
settings stay collapsed by default; open **Settings** only when you want to change
FPS, codec, quality, cursor, system audio, or microphone. Those choices are
remembered across Klippit launches.
`F9` pauses/continues, `F10` stops, and `F11` discards. On Stop, the recording
opens automatically in the normal Klippit editor.

After Stop, Klippit remuxes the finalized native segments into a temporary MKV.
That original remains available until you explicitly save it, discard it, or
finish an edited export and choose what to do with the original. Finalized pause
segments can also be recovered after an interrupted session; the segment that is
actively being written at the instant of a hard process/OS crash may not be.

Once Klippit is open:

1. Scrub, play, or frame-step (`,` / `.`) to your starting frame, then
   click **Mark In** (or press `I`).
2. Do the same for your ending frame, then **Mark Out** (or press `O`).
3. For a multi-section clip, click **+ Add Section** after each range. Each
   committed section keeps its own speed and crop; use its **Crop** button
   to revisit the framing later.
4. Choose MP4, GIF, or APNG, Quality or Target-size mode, resolution,
   subtitles, crop/aspect ratio, and audio as needed.
5. **Export** (or press `Enter`).

Screenshots: the camera button captures a still at the current frame,
matching whatever the subtitle preview toggle (the "CC" button) is
currently showing.

## Settings

The gear icon in the header opens a small settings panel with two
options:

- **mpv keybind** — change which key triggers Klippit from mpv (default
  `c`) without ever editing `clip-trigger.lua` by hand.
- **mpv config folder** — override the default `%APPDATA%\mpv` if your
  mpv uses `portable_config` (a folder next to `mpv.exe` itself) or any
  other nonstandard location.

Click **Save & Reinstall** and both take effect immediately — no restart
needed.

## Known limitations

- Screen recording currently captures a region contained entirely within one
  monitor. The selection is locked after recording starts.
- Live screen capture uses Windows Graphics Capture and the native Windows Media
  encoder. Availability therefore depends on the Windows version, graphics
  driver, and codecs exposed by Windows; the bundled FFmpeg build is no longer
  part of live capture startup.
- VLC's subtitle-language detection and its extension trigger are less
  robust than mpv's — VLC's own scripting API for these is more limited
  and less consistently documented. If something subtitle-related
  behaves oddly on VLC specifically but works fine on mpv, that's a
  known gap, not a general bug.
- External subtitle files (loaded outside the video's own container) are
  only detected via mpv, not VLC.
- Windows only.

## Building from source

```
git clone <this repo>
cd klippit
powershell.exe -ExecutionPolicy Bypass -File scripts/setup-ffmpeg.ps1
cargo tauri dev      # development
cargo tauri build    # produces the installer under
                      # src-tauri/target/release/bundle/
```

Requires the [Rust toolchain](https://rustup.rs) and the
[Tauri CLI](https://tauri.app) (`cargo install tauri-cli`).
`scripts/setup-ffmpeg.ps1` downloads the `ffmpeg`/`ffprobe` binaries
Klippit bundles as sidecars — a one-time step before your first build.

For a detailed record of how this was built, including every bug found
and fixed along the way, see [DEVLOG.md](DEVLOG.md).

## License

MIT — see [LICENSE](LICENSE).

### Screen recorder capture backend

The Windows screen recorder owns a **native Windows Graphics Capture session** in Rust. The capture session is created when the recorder opens and remains alive while the capture region is positioned, while recording, and across Pause/Continue. Pressing Start therefore does not launch FFmpeg or initialize a new desktop-capture process.

At recording time a native Windows Media encoder consumes the most recent captured region at the selected fixed frame rate. If Windows has not produced a new desktop update for a tick, Klippit repeats the latest frame instead of treating a static desktop as a capture failure. Video frames never pass through the webview/JavaScript layer.

Native segments are finalized as MP4 files. FFmpeg remains bundled for Klippit's normal editing/export pipeline and is used after recording only to mux optional WASAPI audio and remux/join finalized segments into the temporary MKV source opened by the editor. `scripts\setup-ffmpeg.cmd` is still the helper for refreshing those bundled FFmpeg/ffprobe sidecars, but live screen capture no longer depends on FFmpeg capture filters.


### Native recorder preview 11b compatibility fix
On Windows builds where capture-border or cursor-toggle properties are unavailable, Klippit now leaves those optional WGC settings at the Windows default instead of failing recorder initialization.

### Klippit 1.3.0 - native screen recorder
- Temporary recordings loaded into the editor now expose **Discard recording** next to **Save original**.
- Pressing **Record** while a temporary recording is open now shows an explicit Save / Discard / Cancel decision instead of silently hitting the backend unsaved-recording guard.
- Discarding from the editor unloads the video first so Windows can delete the temporary recording, then returns Klippit to its empty Browse/Record state.
- Saving before **Record New** releases the temporary source before the recorder cleans its old working directory.
- The yellow outline around the captured display is the Windows Graphics Capture notification border. Preview 11b intentionally leaves Windows' default border behavior intact for compatibility with systems that do not support the optional border-control API.

### Klippit 1.3.0 recorder UI
- The main Record action now occupies the prominent left-side toolbar position instead of repeating the Klippit logo.
- The recorder defaults to a compact status/action view; infrequently changed recording settings live behind a small **Settings** disclosure.
- Recorder FPS, codec, quality, cursor, system-audio, and microphone choices persist across application restarts.
- Settings always reopen collapsed so the recorder remains visually quiet.


### Klippit 1.3.1 - export progress

- Fixed the Output filename row so the format suffix (`.mp4`, `.gif`, `.apng`) always remains visible instead of being pushed outside the panel.
- Added a live export progress bar driven by FFmpeg's `-progress` output rather than a simulated timer.
- Progress is mapped across section extraction, multi-section joining, MP4 quality/two-pass encoding, GIF palette/encode passes, and APNG encoding.
- The UI keeps progress monotonic if an encoder or target-size attempt is retried, and the existing Cancel action still terminates the active export process.

### v1.3.2 recorder workflow polish

- Recorder **Discard** now resets the current take and stays in the recorder at READY; use the title-bar **×** to exit back to Klippit.
- Discarding a temporary recording from the editor now deletes it and returns directly to the recorder.
- The last capture rectangle is remembered when a recording is stopped/closed and reused when the recorder is opened again.
- The main toolbar recorder launcher now shows a larger icon plus an explicit **Record** label.
