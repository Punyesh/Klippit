// Klippit — Tauri backend
//
// This replaces sakuga-enhancer.js's ffmpeg.wasm pipeline with the real
// ffmpeg/ffprobe binaries, invoked as native subprocesses. No virtual
// filesystem, no worker.js patching, no CDN loading — just files on disk
// and full-speed encoding. Most of the *argument-building* logic (crf,
// scale, palette gen/use, subtitle burn-in) ports over conceptually from
// the wasm version; only the "how it's invoked" layer changes.
//
// ffmpeg/ffprobe are called as bundled *sidecar* binaries (via
// tauri-plugin-shell) rather than from the system PATH. That's what lets
// `cargo tauri build` produce a single distributable that never needs
// ffmpeg installed separately — see README.md's "Bundling ffmpeg" section
// for how to actually supply the binaries this expects to find.
//
// STATUS: scaffold. Not compiled/run in this environment (no GUI/display
// or Tauri CLI available here) — verify the tauri-plugin-shell API below
// against the version that lands in your Cargo.lock; sidecar method names
// have shifted across Tauri 2.x point releases.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportParams {
    file_path: String,
    in_time: f64,
    out_time: f64,
    format: String,   // "mp4" | "gif"
    burn_subs: bool,
    mode: String,     // "quality" | "size"
    crf: u32,
    resolution: u32,  // 0 = source
    gif_fps: u32,
    target_mb: f64,
    output_dir: String,
    #[serde(default)]
    file_name: Option<String>, // user-editable filename (no extension) from the Output field
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoMetadata {
    duration: f64,
    fps: f64,
    has_subtitles: bool,
}

// ---------- shared sidecar runner ----------
// Every ffmpeg/ffprobe invocation in this file funnels through here, same
// principle as the old plain-PATH `run()` helper — just backed by the
// bundled binary instead. `app` is auto-injected by Tauri when a command
// declares an `AppHandle` parameter; it costs nothing at the call site.
async fn run_bin(app: &AppHandle, name: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let sidecar = app
        .shell()
        .sidecar(name)
        .map_err(|e| format!("sidecar '{name}' not found in this build: {e}"))?;

    let output = sidecar
        .args(args)
        .output()
        .await
        .map_err(|e| format!("failed to run {name}: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(output.stdout)
}

// ---------- metadata (ffprobe) ----------
#[tauri::command]
async fn get_video_metadata(app: AppHandle, path: String) -> Result<VideoMetadata, String> {
    let stdout = run_bin(&app, "ffprobe", &[
        "-v".into(), "error".into(),
        "-show_entries".into(), "format=duration:stream=r_frame_rate,codec_type".into(),
        "-of".into(), "default=noprint_wrappers=1".into(),
        path,
    ]).await?;

    let text = String::from_utf8_lossy(&stdout);
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

// Strips characters Windows won't allow in a filename, and drops a
// trailing .mp4/.gif if the person typed one — we append the correct
// extension ourselves regardless, so a stray/wrong one shouldn't double up.
fn sanitize_filename_stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let trimmed = if lower.ends_with(".mp4") || lower.ends_with(".gif") {
        &name[..name.len() - 4]
    } else {
        name
    };
    let cleaned: String = trimmed
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "clip".to_string() } else { cleaned.to_string() }
}

// Expands a leading "~" to the user's home directory. "~" is a shell
// convention (bash/zsh expand it before the program ever sees it) — a
// program that receives the literal string, like ffmpeg here, has no idea
// what it means and will just try to open a folder named "~". This is
// what caused "No such file or directory" when exporting with the
// frontend's default output path before the user ever clicked Browse.
fn expand_tilde(path: &str) -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok();
    match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), &home) {
        (Some(rest), Some(h)) => format!("{}/{}", h.trim_end_matches(['/', '\\']), rest),
        _ if path == "~" => home.unwrap_or_else(|| path.to_string()),
        _ => path.to_string(),
    }
}

// ---------- export ----------
#[tauri::command]
async fn export_clip(app: AppHandle, params: ExportParams) -> Result<String, String> {
    let duration = params.out_time - params.in_time;
    if duration <= 0.0 {
        return Err("out point must be after in point".into());
    }

    let output_dir = expand_tilde(&params.output_dir);
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("couldn't create output folder '{output_dir}': {e}"))?;

    let ext = if params.format == "gif" { "gif" } else { "mp4" };

    // A name typed into the Output field is used exactly as given (minus
    // any illegal characters / accidental extension) rather than having
    // our own "_in-out" suffix appended on top of it — the whole point of
    // that field is "this is what the file will be called," not "append
    // to our own naming scheme."
    let stem = match params.file_name.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => sanitize_filename_stem(s),
        _ => std::path::Path::new(&params.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip")
            .to_string(),
    };
    let out_path = format!(
        "{}/{}.{}",
        output_dir.trim_end_matches(['/', '\\']),
        stem,
        ext
    );

    if params.format == "gif" {
        export_gif(&app, &params, duration, &out_path).await?;
    } else {
        export_mp4(&app, &params, duration, &out_path).await?;
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
    // IMPORTANT: -ss before -i (input-side seek, see below) shifts the
    // subtitle filter's own timeline too, since it reads straight from the
    // original file's timestamps — that's what keeps burned subs in sync
    // with a trimmed clip instead of drifting by `in_time` seconds.
    // itsoffset/setpts adjustments are only needed here if subs come from
    // a *separate* external file not already aligned with the video's PTS.
    Some(format!("subtitles='{}'", params.file_path.replace('\'', "\\'")))
}

async fn export_mp4(app: &AppHandle, params: &ExportParams, duration: f64, out_path: &str) -> Result<(), String> {
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

        run_ffmpeg_2pass(app, &params.file_path, params.in_time, duration, vf.as_deref(), video_kbps, audio_kbps, out_path).await
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
        run_bin(app, "ffmpeg", &args).await.map(|_| ())
    }
}

async fn run_ffmpeg_2pass(
    app: &AppHandle, input: &str, start: f64, duration: f64, vf: Option<&str>,
    video_kbps: f64, audio_kbps: f64, out_path: &str,
) -> Result<(), String> {
    let bitrate = format!("{}k", video_kbps as u64);
    let audio = format!("{}k", audio_kbps as u64);

    // 2-pass encoding writes small ffmpeg2pass-*.log files into whatever
    // directory it's told to (via -passlogfile), defaulting to ffmpeg's
    // own working directory if not specified. In `cargo tauri dev`, that
    // default lands inside src-tauri/ — a folder the dev watcher treats
    // as source and rebuilds+relaunches the whole app on any change,
    // which looked like a crash but was actually this. Pointing it at
    // the OS temp dir instead avoids that everywhere, dev or release.
    let passlog_prefix = std::env::temp_dir()
        .join(format!("klippit-2pass-{}", std::process::id()))
        .to_string_lossy()
        .to_string();

    let mut pass1: Vec<String> = vec![
        "-y".into(), "-ss".into(), start.to_string(), "-i".into(), input.into(),
        "-t".into(), duration.to_string(),
        "-c:v".into(), "libx264".into(), "-b:v".into(), bitrate.clone(),
        "-pass".into(), "1".into(), "-passlogfile".into(), passlog_prefix.clone(),
        "-an".into(), "-f".into(), "mp4".into(),
    ];
    if let Some(f) = vf { pass1.push("-vf".into()); pass1.push(f.into()); }
    #[cfg(windows)] pass1.push("NUL".into());
    #[cfg(not(windows))] pass1.push("/dev/null".into());
    run_bin(app, "ffmpeg", &pass1).await?;

    let mut pass2: Vec<String> = vec![
        "-y".into(), "-ss".into(), start.to_string(), "-i".into(), input.into(),
        "-t".into(), duration.to_string(),
        "-c:v".into(), "libx264".into(), "-b:v".into(), bitrate,
        "-pass".into(), "2".into(), "-passlogfile".into(), passlog_prefix.clone(),
        "-c:a".into(), "aac".into(), "-b:a".into(), audio,
    ];
    if let Some(f) = vf { pass2.push("-vf".into()); pass2.push(f.into()); }
    pass2.push(out_path.into());
    let result = run_bin(app, "ffmpeg", &pass2).await.map(|_| ());

    // Best-effort cleanup — leftover pass-log files in the temp dir are
    // harmless either way, so a failed removal here doesn't fail the export.
    let _ = std::fs::remove_file(format!("{passlog_prefix}-0.log"));
    let _ = std::fs::remove_file(format!("{passlog_prefix}-0.log.mbtree"));

    result
}

async fn export_gif(app: &AppHandle, params: &ExportParams, duration: f64, out_path: &str) -> Result<(), String> {
    // Two-pass palette gen/use — same technique as the wasm version, just
    // real ffmpeg instead of a wasm build. Target-size mode iterates width/
    // fps rather than a bitrate knob, since GIF has none.
    let (mut width, mut fps) = if params.mode == "size" {
        (480u32, params.gif_fps.max(10))
    } else {
        (if params.resolution == 0 { 480 } else { params.resolution }, params.gif_fps)
    };

    for attempt in 0..3 {
        encode_gif_attempt(app, params, duration, width, fps, out_path).await?;
        if params.mode != "size" { break; }

        let size_mb = std::fs::metadata(out_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
        if size_mb <= params.target_mb || attempt == 2 { break; }
        // Overshot — back off width and fps and retry once or twice.
        width = (width as f64 * 0.8) as u32;
        fps = (fps as f64 * 0.85).max(8.0) as u32;
    }
    Ok(())
}

async fn encode_gif_attempt(app: &AppHandle, params: &ExportParams, duration: f64, width: u32, fps: u32, out_path: &str) -> Result<(), String> {
    let mut base_filters = vec![format!("fps={fps}"), format!("scale={width}:-2:flags=lanczos")];
    if let Some(f) = subtitle_filter(params) { base_filters.push(f); }
    let base = base_filters.join(",");

    let palette = format!("{out_path}.palette.png");

    run_bin(app, "ffmpeg", &[
        "-y".into(), "-ss".into(), params.in_time.to_string(), "-i".into(), params.file_path.clone(),
        "-t".into(), duration.to_string(),
        "-vf".into(), format!("{base},palettegen"),
        palette.clone(),
    ]).await?;

    run_bin(app, "ffmpeg", &[
        "-y".into(), "-ss".into(), params.in_time.to_string(), "-i".into(), params.file_path.clone(),
        "-t".into(), duration.to_string(),
        "-i".into(), palette.clone(),
        "-lavfi".into(), format!("{base}[x];[x][1:v]paletteuse=dither=bayer"),
        out_path.into(),
    ]).await?;

    let _ = std::fs::remove_file(&palette);
    Ok(())
}

// ---------- screenshots ----------
// Screenshots are handled by mpv itself (`screenshot video` / `screenshot
// subtitles`) — see mpv-scripts/clip-trigger.lua. This command is a
// fallback for extracting a still via ffmpeg if a screenshot is ever
// requested outside of an active mpv session.
#[tauri::command]
async fn extract_frame(app: AppHandle, path: String, at: f64, burn_subs: bool, out_path: String) -> Result<String, String> {
    let out_path = expand_tilde(&out_path);
    if let Some(parent) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("couldn't create output folder: {e}"))?;
    }
    let mut args: Vec<String> = vec![
        "-y".into(), "-ss".into(), at.to_string(), "-i".into(), path.clone(),
        "-frames:v".into(), "1".into(),
    ];
    if burn_subs {
        args.push("-vf".into());
        args.push(format!("subtitles='{}'", path.replace('\'', "\\'")));
    }
    args.push(out_path.clone());
    run_bin(&app, "ffmpeg", &args).await?;
    Ok(out_path)
}

// Opens a small dedicated window playing the given file, with native
// <video controls> — used by the Play button so review happens inside
// Klippit rather than handing off to the OS default media player.
#[tauri::command]
async fn open_review_window(app: AppHandle, path: String) -> Result<(), String> {
    // If a review window is already open (e.g. Play clicked twice), close
    // it first rather than erroring on a duplicate window label.
    if let Some(existing) = app.get_webview_window("review") {
        let _ = existing.close();
    }

    let path_json = serde_json::to_string(&path).map_err(|e| e.to_string())?;
    let script = format!("window.__KLIPPIT_REVIEW_PATH__ = {path_json};");

    tauri::WebviewWindowBuilder::new(&app, "review", tauri::WebviewUrl::App("review.html".into()))
        .title("Klippit — Preview")
        .inner_size(640.0, 480.0)
        .initialization_script(&script)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(())
}

// Reads --init <json> from argv (passed by clip-trigger.lua, or by hand
// for testing without mpv at all — see README) and returns it so it can
// be injected into the webview before app.js runs.
// Extracted so the same "find --init <json> in an argv list" logic works
// both for this process's own launch args (main()) and for a second
// launch's args handed to us by the single-instance plugin below.
fn extract_init_arg(args: &[String]) -> Option<String> {
    args.iter()
        .position(|a| a == "--init")
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn read_init_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    extract_init_arg(&args)
}

fn main() {
    let init_json = read_init_arg();

    tauri::Builder::default()
        // Must be registered first — this plugin needs to be able to
        // short-circuit everything else when it detects a second launch,
        // per Tauri's own docs. When mpv's trigger key is pressed while a
        // Klippit window is already open, the SECOND process's argv gets
        // handed to this closure running inside the FIRST (already
        // running) process, and the second process exits immediately
        // without ever reaching .setup() below or creating its own
        // window — so this is genuinely "reuse the existing window," not
        // "hide a second one."
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(json) = extract_init_arg(&argv) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.eval(&format!("window.applyInit && window.applyInit({json});"));
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            // The window is built here programmatically, rather than
            // declared in tauri.conf.json, specifically so
            // initialization_script can run — that's the mechanism that
            // guarantees window.__KLIPPIT_INIT__ exists BEFORE app.js's
            // own top-level code runs (a plain window.eval after the
            // window already exists can't make that guarantee, since
            // app.js may have already started executing by then).
            let mut builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Klippit")
            .inner_size(480.0, 720.0)
            .min_inner_size(420.0, 660.0)
            .resizable(true)
            .always_on_top(true);

            if let Some(json) = &init_json {
                let script = format!("window.__KLIPPIT_INIT__ = {json};");
                builder = builder.initialization_script(&script);
            }

            builder.build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_video_metadata,
            export_clip,
            extract_frame,
            open_review_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
