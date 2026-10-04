# Klippit

A lightweight Windows tool for making video clips, GIFs, APNGs, screenshots, and screen recordings. Open a file directly, trigger Klippit from **mpv** or **VLC**, or record a region of your screen and edit it immediately.

![platform](https://img.shields.io/badge/platform-Windows-blue)
![license](https://img.shields.io/badge/license-MIT-green)

## Features

- **Native screen recording** — record a resizable region on one monitor with Start, Pause/Continue, Stop, and Discard controls. Finished recordings open directly in Klippit for editing.
- **Persistent recorder settings** — choose 24/30/60/120 FPS, H.264 or HEVC, quality, cursor, system audio, and microphone. Your choices are remembered.
- **mpv and VLC integration** — launch Klippit from the player at the current playback position, or use Klippit completely standalone.
- **Precise clipping** — frame-by-frame stepping with explicit Mark In / Mark Out points.
- **Multi-section clips** — combine multiple ranges with independent speed and crop/framing for each section.
- **MP4, GIF, and APNG export** — Quality and Target-size workflows with a live export progress bar.
- **Crop and aspect-ratio controls** — including per-section pan/zoom while keeping a consistent project output ratio.
- **Subtitle burn-in and preview** — supports embedded subtitle tracks and external subtitles opened through mpv.
- **Screenshots** — capture the current frame with or without the subtitle preview.
- **Audio controls** — mute exported MP4s and optionally capture system audio or microphone audio when screen recording.

## Installing

Download the latest installer from the [Releases page](../../releases) and run it. Klippit requires **Windows**.

mpv and VLC are optional. They are only needed for player integration; Klippit can open files and record the screen without either one installed.

## Using Klippit

**Open a file:** launch Klippit and drag a video onto it, or click **Browse for a file…**.

**From mpv:** while a video is playing, press `c` to open Klippit at the current playback position.

**From VLC:** open a video, then use **View → Extensions → Klippit**.

**Screen recording:** click **Record** in Klippit's main toolbar. Move and resize the capture region, then press **Start** or `F9`. Settings are collapsed by default and remembered between launches.

Recorder shortcuts:

- `F9` — Start / Pause / Continue
- `F10` — Stop
- `F11` — Discard current take

Stopping a recording opens it automatically in Klippit. Discarding a take keeps the recorder ready for another recording; discarding a stopped temporary recording from the editor returns directly to the recorder.

### Editing and exporting

1. Move to the starting frame and click **Mark In** (`I`).
2. Move to the ending frame and click **Mark Out** (`O`).
3. Optionally use **+ Add Section** to combine additional ranges.
4. Choose MP4, GIF, or APNG and adjust crop, speed, subtitles, size/quality, and audio as needed.
5. Click **Export** (`Enter`). Klippit shows live export progress while processing.

Use `,` and `.` for frame-by-frame stepping and `Space` for play/pause.

## Settings

The gear icon contains Klippit's mpv integration settings:

- **mpv keybind** — change the key used to trigger Klippit from mpv.
- **mpv config folder** — override the default `%APPDATA%\mpv` location for portable or custom mpv setups.

Recorder-specific settings live inside the recorder's collapsed **Settings** menu and persist automatically.

## Notes and limitations

- Screen recording is limited to a region contained within one monitor, and the capture region locks while recording.
- Klippit uses native **Windows Graphics Capture** for live screen capture. Windows may show its own yellow capture indicator around the captured display; this is normal.
- The actively written recording segment may be lost if Windows or Klippit crashes before that segment is finalized. Previously finalized pause segments can be recovered.
- External subtitle files are detected through mpv, not VLC.
- VLC integration is more limited than mpv for subtitle metadata and triggering.

## Building from source

```powershell
git clone <this repo>
cd klippit
.\scripts\setup-ffmpeg.cmd
cargo tauri dev
# or
cargo tauri build
```

Requires the [Rust toolchain](https://rustup.rs) and the [Tauri CLI](https://tauri.app) (`cargo install tauri-cli`). The setup script downloads the FFmpeg/ffprobe sidecars used by Klippit's editing and export pipeline; live screen capture itself is native Windows capture.

For implementation history and detailed development notes, see [DEVLOG.md](DEVLOG.md).

## License

MIT — see [LICENSE](LICENSE).
