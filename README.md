# Klippit

A frame-accurate video clip, GIF, and screenshot exporter, triggered by a
single keypress while you're watching in **mpv** or **VLC**. No manual
timestamp-copying, no separate video editor — mark your in and out
points on the spot, export, done.

![platform](https://img.shields.io/badge/platform-Windows-blue)
![license](https://img.shields.io/badge/license-MIT-green)

## Features

- **Trigger from mpv or VLC** — press `c` in mpv, or use the "Klippit"
  extension in VLC's View menu, right at the moment you want to start
  clipping.
- **Explicit Mark In / Mark Out** — scrub or frame-step to the exact
  frame, then mark it. No dragging tiny handles, no guessing.
- **MP4 or GIF export**, each with two modes:
  - **Quality** — pick a CRF value, keep the source resolution or scale
    down.
  - **Target size** — give it a size in MB, it 2-pass encodes to hit
    that target.
- **Subtitle burn-in**, matched correctly against multi-track files (by
  language) and external subtitle files (mpv only), with a live preview
  overlay you can toggle on/off while editing.
- **Screenshots**, with or without subtitles, matching whatever the live
  preview is currently showing.
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

**Requires:** Windows, and either [mpv](https://mpv.io) or
[VLC](https://www.videolan.org/vlc/) already installed (or installed any
time after — order doesn't matter). Klippit is a companion tool for one
of these; it doesn't include a media player itself.

## Using it

**From mpv:** while a video is playing, press `c` at the moment you want
your clip to start. Klippit opens, seeded to that exact timestamp.

**From VLC:** open a video, then **View → Extensions → Klippit**.

**Or open Klippit directly** (Start Menu, desktop shortcut) and drop a
video file onto it, or click **Browse for a file…**.

Once Klippit is open:

1. Scrub, play, or frame-step (`,` / `.`) to your starting frame, then
   click **Mark In** (or press `I`).
2. Do the same for your ending frame, then **Mark Out** (or press `O`).
3. Choose MP4 or GIF, Quality or Target-size mode, resolution, subtitles,
   and audio as needed.
4. **Export** (or press `Enter`).

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
