// Klippit — Tauri backend
//
// This replaces sakuga-enhancer.js's ffmpeg.wasm pipeline with the real
// ffmpeg/ffprobe binaries, invoked as native subprocesses. No virtual
// filesystem, no worker.js patching, no CDN loading — just files on disk
// and full-speed encoding. Most of the *argument-building* logic (crf,
// scale, palette gen/use, subtitle burn-in) ports over conceptually from
// the wasm version; only the "how it's invoked" layer changes.
//
// STATUS: scaffold. Not compiled/run in this environment (no GUI/display
// or Tauri CLI available here) — verify `ffmpeg`/`ffprobe` are on PATH and
// adjust paths for your OS before building.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Deserialize)]
struct ExportParams {
    file_path: String,
    in_time: f64,
    out_time: f64,
    format: String,     // "mp4" | "gif"
    burn_subs: bool,
    mode: String,        // "quality" | "size"
    crf: u32,
    resolution: u32,      // 0 = source
    gif_fps: u32,
    target_mb: f64,
    output_dir: String,
}

#[derive(Debug, Serialize)]
struct VideoMetadata {
    duration: f64,
    fps: f64,
    has_subtitles: bool,
}

// ---------- metadata (ffprobe) ----------
#[tauri::command]
fn get_video_metadata(path: String) -> Result<VideoMetadata, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration:stream=r_frame_rate,codec_type",
            "-of", "default=noprint_wrappers=1",
            &path,
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "ffprobe exited with error: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut duration = 0.0;
    let mut fps = 24.0;
    let mut has_subtitles = false;

    for line in text.lines() {
        if let Some(v) = line.strip_prefix("duration=") {
            duration = v.trim().parse().unwrap_or(0.0);
        } else if let Some(v) = line.strip_prefix("r_frame_rate=") {
            // e.g. "24000/1001" -> 23.976
            if let Some((num, den)) = v.split_once('/') {
                let n: f64 = num.parse().unwrap_or(24.0);
                let d: f64 = den.parse().unwrap_or(1.0);
                if d != 0.0 { fps = n / d; }
            }
        } else if line.contains("codec_type=subtitle") {
            has_subtitles = true;
        }
    }

    Ok(VideoMetadata { duration, fps, has_subtitles })
}

// ---------- export ----------
#[tauri::command]
fn export_clip(params: ExportParams) -> Result<String, String> {
    let duration = params.out_time - params.in_time;
    if duration <= 0.0 {
        return Err("out point must be after in point".into());
    }

    let ext = if params.format == "gif" { "gif" } else { "mp4" };
    let file_stem = std::path::Path::new(&params.file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("clip");
    let out_path = format!(
        "{}/{}_{:.2}-{:.2}.{}",
        params.output_dir.trim_end_matches('/'),
        file_stem,
        params.in_time,
        params.out_time,
        ext
    );

    if params.format == "gif" {
        export_gif(&params, duration, &out_path)?;
    } else {
        export_mp4(&params, duration, &out_path)?;
    }

    Ok(out_path)
}

fn scale_filter(resolution: u32) -> Option<String> {
    if resolution == 0 { return None; }
    // Scale to target height, preserve aspect ratio, even dimensions
    // (required by most encoders).
    Some(format!("scale=-2:{resolution}"))
}

fn subtitle_filter(params: &ExportParams) -> Option<String> {
    if !params.burn_subs { return None; }
    // IMPORTANT: -ss before -i (see the input-seek note below) shifts the
    // subtitle filter's own timeline too, since it reads straight from the
    // original file's timestamps — that's what keeps burned subs in sync
    // with a trimmed clip instead of drifting by `in_time` seconds.
    // itsoffset/setpts adjustments are only needed here if subs come from
    // a *separate* external file not already aligned with the video's PTS.
    Some(format!("subtitles='{}'", params.file_path.replace('\'', "\\'")))
}

fn export_mp4(params: &ExportParams, duration: f64, out_path: &str) -> Result<(), String> {
    let mut filters: Vec<String> = vec![];
    if let Some(f) = scale_filter(params.resolution) { filters.push(f); }
    if let Some(f) = subtitle_filter(params) { filters.push(f); }
    let vf = if filters.is_empty() { None } else { Some(filters.join(",")) };

    if params.mode == "size" {
        // Target-size mode: compute bitrate from target_size / duration,
        // reserve a fixed slice for audio, 2-pass encode to hit it reliably.
        let target_bits = params.target_mb * 8_000_000.0;
        let audio_kbps = 128.0;
        let video_kbps = ((target_bits / duration / 1000.0) - audio_kbps).max(200.0);

        run_ffmpeg_2pass(&params.file_path, params.in_time, duration, vf.as_deref(), video_kbps, audio_kbps, out_path)
    } else {
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-ss".into(), params.in_time.to_string(), // input-side seek: fast + accurate enough for most clips
            "-i".into(), params.file_path.clone(),
            "-t".into(), duration.to_string(),
            "-crf".into(), params.crf.to_string(),
            "-preset".into(), "medium".into(),
            "-c:a".into(), "aac".into(), "-b:a".into(), "128k".into(),
        ];
        if let Some(f) = &vf { args.push("-vf".into()); args.push(f.clone()); }
        args.push(out_path.into());
        run(&args)
    }
}

fn run_ffmpeg_2pass(
    input: &str, start: f64, duration: f64, vf: Option<&str>,
    video_kbps: f64, audio_kbps: f64, out_path: &str,
) -> Result<(), String> {
    let bitrate = format!("{}k", video_kbps as u64);
    let audio = format!("{}k", audio_kbps as u64);

    let mut pass1: Vec<String> = vec![
        "-y".into(), "-ss".into(), start.to_string(), "-i".into(), input.into(),
        "-t".into(), duration.to_string(),
        "-c:v".into(), "libx264".into(), "-b:v".into(), bitrate.clone(),
        "-pass".into(), "1".into(), "-an".into(), "-f".into(), "mp4".into(),
    ];
    if let Some(f) = vf { pass1.push("-vf".into()); pass1.push(f.into()); }
    #[cfg(windows)] pass1.push("NUL".into());
    #[cfg(not(windows))] pass1.push("/dev/null".into());
    run(&pass1)?;

    let mut pass2: Vec<String> = vec![
        "-y".into(), "-ss".into(), start.to_string(), "-i".into(), input.into(),
        "-t".into(), duration.to_string(),
        "-c:v".into(), "libx264".into(), "-b:v".into(), bitrate,
        "-pass".into(), "2".into(),
        "-c:a".into(), "aac".into(), "-b:a".into(), audio,
    ];
    if let Some(f) = vf { pass2.push("-vf".into()); pass2.push(f.into()); }
    pass2.push(out_path.into());
    run(&pass2)
}

fn export_gif(params: &ExportParams, duration: f64, out_path: &str) -> Result<(), String> {
    // Two-pass palette gen/use — same technique as the wasm version, just
    // real ffmpeg instead of a wasm build. Target-size mode iterates width/
    // fps rather than a bitrate knob, since GIF has none.
    let (mut width, mut fps) = if params.mode == "size" {
        (480u32, params.gif_fps.max(10))
    } else {
        (if params.resolution == 0 { 480 } else { params.resolution }, params.gif_fps)
    };

    for attempt in 0..3 {
        encode_gif_attempt(params, duration, width, fps, out_path)?;
        if params.mode != "size" { break; }

        let size_mb = std::fs::metadata(out_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
        if size_mb <= params.target_mb || attempt == 2 { break; }
        // Overshot — back off width and fps and retry once or twice.
        width = (width as f64 * 0.8) as u32;
        fps = (fps as f64 * 0.85).max(8.0) as u32;
    }
    Ok(())
}

fn encode_gif_attempt(params: &ExportParams, duration: f64, width: u32, fps: u32, out_path: &str) -> Result<(), String> {
    let mut base_filters = vec![format!("fps={fps}"), format!("scale={width}:-2:flags=lanczos")];
    if let Some(f) = subtitle_filter(params) { base_filters.push(f); }
    let base = base_filters.join(",");

    let palette = format!("{out_path}.palette.png");

    run(&[
        "-y".into(), "-ss".into(), params.in_time.to_string(), "-i".into(), params.file_path.clone(),
        "-t".into(), duration.to_string(),
        "-vf".into(), format!("{base},palettegen"),
        palette.clone(),
    ])?;

    run(&[
        "-y".into(), "-ss".into(), params.in_time.to_string(), "-i".into(), params.file_path.clone(),
        "-t".into(), duration.to_string(),
        "-i".into(), palette.clone(),
        "-lavfi".into(), format!("{base}[x];[x][1:v]paletteuse=dither=bayer"),
        out_path.into(),
    ])?;

    let _ = std::fs::remove_file(&palette);
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

// ---------- screenshots ----------
// Screenshots are handled by mpv itself (`screenshot video` / `screenshot
// subtitles`) — see mpv-scripts/clip-trigger.lua. This command is a
// fallback for extracting a still via ffmpeg if a screenshot is ever
// requested outside of an active mpv session.
#[tauri::command]
fn extract_frame(path: String, at: f64, burn_subs: bool, out_path: String) -> Result<String, String> {
    let mut args: Vec<String> = vec![
        "-y".into(), "-ss".into(), at.to_string(), "-i".into(), path.clone(),
        "-frames:v".into(), "1".into(),
    ];
    if burn_subs {
        args.push("-vf".into());
        args.push(format!("subtitles='{}'", path.replace('\'', "\\'")));
    }
    args.push(out_path.clone());
    run(&args)?;
    Ok(out_path)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_video_metadata,
            export_clip,
            extract_frame
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
