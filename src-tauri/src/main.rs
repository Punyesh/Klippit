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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    output_path: String,
    // "libx264" | "h264_nvenc" | "h264_amf" | "h264_qsv" | "gif" — lets
    // the frontend tell the person whether GPU encoding they opted into
    // actually happened, or silently fell back to the CPU encoder.
    encoder_used: String,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct SectionParam {
    in_time: f64,
    out_time: f64,
    #[serde(default = "default_speed")]
    speed: f64,
}
fn default_speed() -> f64 { 1.0 }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportParams {
    file_path: String,
    // One or more (start,end) ranges from the source, combined in the
    // order given into a single output. The frontend always sends at
    // least one entry (the current Mark In/Out range) — multi-section
    // export isn't a separate mode, just this array having more than
    // one entry. Replaces the old single in_time/out_time fields
    // entirely, rather than keeping both and special-casing the
    // single-section path — every export, one section or several, goes
    // through the same extract-each-then-concat logic (see
    // extract_and_concat_sections), with N=1 just skipping the actual
    // concat step since there's nothing to join.
    sections: Vec<SectionParam>,
    format: String,   // "mp4" | "gif" | "apng"
    burn_subs: bool,
    mode: String,     // "quality" | "size"
    crf: u32,
    resolution: u32,  // 0 = source
    gif_fps: u32,
    target_mb: f64,
    output_dir: String,
    #[serde(default)]
    file_name: Option<String>, // user-editable filename (no extension) from the Output field
    #[serde(default)]
    source_width: u32,  // from get_video_metadata — needed so the subtitles
    #[serde(default)]   // filter can be told original_size explicitly rather
    source_height: u32, // than relying on auto-detection, which fails when
                         // subtitles is applied after a scale filter
    #[serde(default)]
    subtitle_lang: Option<String>,        // from mpv's current-tracks/sub/lang, for
    #[serde(default)]                     // picking the right embedded track when
    subtitle_external_file: Option<String>, // there are several; external_file (an
                                             // externally-loaded subtitle file, not
                                             // embedded in the container) takes
                                             // priority over lang matching when set
    #[serde(default)]
    mute_audio: bool, // strip audio entirely (-an) rather than encoding it;
                       // meaningless for GIF (never has audio), UI hides the
                       // toggle in that case
    #[serde(default)]
    use_gpu: bool, // opt-in hardware encoding for MP4 Quality mode only —
                   // see encode_quality_mode for the fallback logic. Not
                   // offered for Target-size mode (2-pass hardware support
                   // is far less consistent across encoders/drivers) or
                   // GIF (palette-based, no H.264 encoder involved at all)
    // Static crop applied uniformly to the whole clip, in SOURCE VIDEO
    // PIXEL coordinates. All four Some or all four None — the frontend
    // only ever sends real numbers when crop is enabled, null otherwise.
    #[serde(default)]
    crop_x: Option<u32>,
    #[serde(default)]
    crop_y: Option<u32>,
    #[serde(default)]
    crop_width: Option<u32>,
    #[serde(default)]
    crop_height: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoMetadata {
    duration: f64,
    fps: f64,
    has_subtitles: bool,
    width: u32,
    height: u32,
}

// ---------- shared sidecar runner ----------
// Every ffmpeg/ffprobe invocation in this file funnels through here, same
// principle as the old plain-PATH `run()` helper — just backed by the
// bundled binary instead. `app` is auto-injected by Tauri when a command
// declares an `AppHandle` parameter; it costs nothing at the call site.
//
// `cwd`: only ever Some(...) for the subtitle-burn encode step. After two
// different failed attempts at escaping a Windows drive-letter colon
// inside a `-vf` filter string (single-quote-wrapping, then backslash-
// escaping — both confirmed failing on real files with a "No option
// name near ..." error each time, in slightly different ways), this
// sidesteps the whole class of problem: run ffmpeg with its working
// directory set to the subtitle temp folder and reference the file by
// its bare filename ("subs.ass") instead — no path, no colon, nothing
// left to escape at all.
//
// UNVERIFIED: `.current_dir(dir)` on tauri-plugin-shell's sidecar
// builder is written from its documented std::process::Command-mirroring
// API, not yet confirmed against a real compile. If this doesn't exist
// under that exact name, check that crate's current docs for the actual
// method — the rest of this fix's logic (bare filename + matching cwd)
// stays correct regardless of what that one method call ends up being.
// Appends every ffmpeg/ffprobe invocation's exact command line to
// %TEMP%\klippit-ffmpeg.log — added specifically to diagnose a real
// report of GIF export hanging with unexpectedly low, sustained CPU
// usage (16%, not the near-100% burst you'd expect for a genuine few-
// second encode, and not 0% either, so not a hard deadlock) — the
// leading theory is a seek/duration argument not correctly limiting one
// of the GIF pipeline's two ffmpeg passes to the intended short window,
// causing it to decode from much further back in the file than
// intended. This log makes that checkable directly rather than guessed
// at from code alone. Best-effort — a logging failure never affects the
// actual export.
fn log_command(name: &str, args: &[String], cwd: Option<&std::path::Path>) {
    use std::io::Write;
    let log_path = std::env::temp_dir().join("klippit-ffmpeg.log");
    let cwd_note = cwd.map(|c| format!(" (cwd: {})", c.display())).unwrap_or_default();
    let line = format!("{name} {}{cwd_note}\n", args.join(" "));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = f.write_all(line.as_bytes());
    }
}

// ---------- export cancellation ----------
// Tracks the currently-running ffmpeg/ffprobe child process (if any) so
// a separate cancel_export command can kill it. A single slot is enough:
// one export runs its ffmpeg steps strictly sequentially, never
// concurrently, so a new run_bin call simply replaces whatever was here
// before it. Killing whichever step happens to be running when cancel
// is pressed is exactly the right behavior — the resulting error
// naturally aborts the rest of the export sequence via the same ?
// propagation already used everywhere, no separate cancellation
// plumbing needed through export_gif/export_mp4/etc.
struct ExportState {
    current_child: std::sync::Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
    cancelled: std::sync::atomic::AtomicBool,
}
impl Default for ExportState {
    fn default() -> Self {
        ExportState {
            current_child: std::sync::Mutex::new(None),
            cancelled: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[tauri::command]
fn cancel_export(state: tauri::State<ExportState>) -> Result<(), String> {
    state.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut guard = state.current_child.lock().map_err(|e| e.to_string())?;
    if let Some(child) = guard.take() {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

// UNVERIFIED: this rewrite uses genuinely new API surface for this
// project — .spawn() / CommandEvent / CommandChild have never been used
// here before (every prior ffmpeg/ffprobe call used the simpler
// .output(), which doesn't expose a handle you can kill mid-flight).
// Written from tauri-plugin-shell's documented pattern for exactly this
// need, not confirmed against a real build. Signature and return type
// are deliberately unchanged from the old .output()-based version, so
// if this needs adjusting, no other call site in the whole file is
// affected — every existing caller keeps working exactly as before.
async fn run_bin(app: &AppHandle, name: &str, args: &[String], cwd: Option<&std::path::Path>) -> Result<Vec<u8>, String> {
    log_command(name, args, cwd);
    let sidecar = app
        .shell()
        .sidecar(name)
        .map_err(|e| format!("sidecar '{name}' not found in this build: {e}"))?;

    let sidecar = match cwd {
        Some(dir) => sidecar.current_dir(dir),
        None => sidecar,
    };

    let (mut rx, child) = sidecar
        .args(args)
        .spawn()
        .map_err(|e| format!("failed to run {name}: {e}"))?;

    let state = app.state::<ExportState>();
    *state.current_child.lock().map_err(|e| e.to_string())? = Some(child);

    use tauri_plugin_shell::process::CommandEvent;
    let mut stdout: Vec<u8> = vec![];
    let mut stderr: Vec<u8> = vec![];
    let mut exit_success = false;

    while let Some(event) = rx.recv().await {
        match event {
            // CommandEvent::Stdout/Stderr deliver data per line (the
            // trailing newline stripped by the underlying line-reader),
            // so a newline has to be reinserted between accumulated
            // chunks here — otherwise multi-line output collapses into
            // one run-on line with no separators at all. This was a
            // real, confirmed regression from the .output()-to-.spawn()
            // switch made for cancel_export: extract_subtitles_to's
            // ffprobe stream-probing step parses run_bin's stdout with
            // .lines() to find the subtitle stream index, and without
            // this fix that parsing silently breaks — explaining
            // subtitle burn-in failing across both MP4 and GIF,
            // regardless of crop, exactly as reported. .output() never
            // had this problem since it always returned the complete,
            // untouched byte stream in one piece.
            CommandEvent::Stdout(line) => { stdout.extend_from_slice(&line); stdout.push(b'\n'); }
            CommandEvent::Stderr(line) => { stderr.extend_from_slice(&line); stderr.push(b'\n'); }
            CommandEvent::Terminated(payload) => {
                exit_success = payload.code == Some(0);
            }
            CommandEvent::Error(err) => {
                *state.current_child.lock().map_err(|e| e.to_string())? = None;
                return Err(format!("failed to run {name}: {err}"));
            }
            _ => {}
        }
    }

    let was_cancelled = state.cancelled.swap(false, std::sync::atomic::Ordering::SeqCst);
    *state.current_child.lock().map_err(|e| e.to_string())? = None;
    if was_cancelled {
        return Err("cancelled".to_string());
    }

    if !exit_success {
        return Err(String::from_utf8_lossy(&stderr).to_string());
    }
    Ok(stdout)
}

// ---------- metadata (ffprobe) ----------
#[tauri::command]
async fn get_video_metadata(app: AppHandle, path: String) -> Result<VideoMetadata, String> {
    let stdout = run_bin(&app, "ffprobe", &[
        "-v".into(), "error".into(),
        "-show_entries".into(), "format=duration:stream=r_frame_rate,codec_type,width,height".into(),
        "-of".into(), "default=noprint_wrappers=1".into(),
        path,
    ], None).await?;

    let text = String::from_utf8_lossy(&stdout);
    let mut duration = 0.0;
    let mut fps = 24.0;
    let mut has_subtitles = false;
    let mut width = 0u32;
    let mut height = 0u32;

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
        } else if let Some(v) = line.strip_prefix("width=") {
            // Only the first video stream's width/height — a file with an
            // attached cover-art image (itself reported as a "video"
            // stream by ffprobe) could otherwise clobber the real
            // dimensions if it happened to come first, but that's an
            // edge case not handled here.
            if width == 0 { width = v.trim().parse().unwrap_or(0); }
        } else if let Some(v) = line.strip_prefix("height=") {
            if height == 0 { height = v.trim().parse().unwrap_or(0); }
        }
    }

    Ok(VideoMetadata { duration, fps, has_subtitles, width, height })
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
async fn export_clip(app: AppHandle, params: ExportParams) -> Result<ExportResult, String> {
    if params.sections.is_empty() {
        return Err("no section to export".into());
    }
    for s in &params.sections {
        if s.out_time <= s.in_time {
            return Err("out point must be after in point".into());
        }
    }
    // Each section's contribution to the final combined duration is its
    // raw range divided by its own speed — a 5s section at 2x plays back
    // in 2.5s, so it only contributes 2.5s toward the total the rest of
    // the pipeline (encoding decisions, the UI's own duration readout)
    // needs to reason about.
    let duration: f64 = params.sections.iter().map(|s| (s.out_time - s.in_time) / s.speed).sum();

    // Logged specifically to help diagnose a report (not yet
    // reproducible here) of GIF export ignoring the Out marker and
    // clipping to the end of the video instead. This records exactly
    // what the frontend actually sent for every section — so if this
    // happens again, the diagnostic log shows definitively whether the
    // wrong values were ever received here at all (a frontend-side
    // issue) versus received correctly but mishandled somewhere further
    // down the export pipeline (a backend issue) — a real distinction
    // that's impossible to tell apart from the symptom alone.
    let mut log_fields: Vec<String> = vec![
        format!("format={}", params.format),
        format!("mode={}", params.mode),
        format!("sectionCount={}", params.sections.len()),
        format!("totalDuration={duration}"),
    ];
    for (i, s) in params.sections.iter().enumerate() {
        log_fields.push(format!("section[{i}]=({}, {}, speed={})", s.in_time, s.out_time, s.speed));
    }
    log_command("export_clip params", &log_fields, None);

    let output_dir = expand_tilde(&params.output_dir);
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("couldn't create output folder '{output_dir}': {e}"))?;

    let ext = match params.format.as_str() {
        "gif" => "gif",
        "apng" => "apng",
        _ => "mp4",
    };

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

    // Subtitle original_size uses the CROPPED dimensions when crop is
    // active, not the original source dimensions — subtitles are burned
    // in after crop in each section's own extraction filter chain (see
    // extract_and_concat_sections), so they need to be positioned/sized
    // relative to the frame they're actually being drawn onto, not the
    // pre-crop source. Subtitle extraction itself now happens per
    // section (inside extract_and_concat_sections), not once upfront
    // here — each section needs its own subtitle timestamps rebased to
    // that section's own start, not the source's original timeline.
    let (effective_width, effective_height) = match (params.crop_width, params.crop_height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => (params.source_width, params.source_height),
    };

    let encoder_used = if params.format == "gif" {
        export_gif(&app, &params, &out_path, effective_width, effective_height).await?;
        "gif".to_string()
    } else if params.format == "apng" {
        export_apng(&app, &params, &out_path, effective_width, effective_height).await?;
        "apng".to_string()
    } else {
        export_mp4(&app, &params, duration, &out_path, effective_width, effective_height).await?
    };

    Ok(ExportResult { output_path: out_path, encoder_used })
}

// Extracts each section separately (crop and, if active, that section's
// own subtitle range rebased to start at 0 — see extract_subtitles_to's
// trim parameter — burned in during extraction), then concatenates them
// via ffmpeg's concat demuxer into one combined file. A single section
// just returns its own extracted file directly, skipping the concat
// step since there's nothing to join — this is the same code path for
// one section or several, not a special case bolted on top.
//
// Every section is re-encoded (never stream-copied) for the same
// frame-accuracy reason already established elsewhere in this file:
// stream copy can only cut at keyframes, silently shifting the actual
// requested start. All sections use identical codec settings
// specifically so the final concat step CAN safely use stream copy
// (-c copy) — mismatched codecs/parameters across concat inputs is a
// real way for that step to fail or produce a broken file.
async fn extract_and_concat_sections(
    app: &AppHandle, params: &ExportParams, effective_width: u32, effective_height: u32,
    keep_audio: bool, temp_prefix: &str,
) -> Result<std::path::PathBuf, String> {
    let mut segment_paths: Vec<std::path::PathBuf> = vec![];

    for (i, section) in params.sections.iter().enumerate() {
        let seg_duration = section.out_time - section.in_time;
        let seg_path = std::env::temp_dir().join(format!("klippit-{temp_prefix}-seg{i}-{}.mkv", std::process::id()));
        let speed_active = (section.speed - 1.0).abs() > 0.0001;

        let mut filters: Vec<String> = vec![];
        // When speed is active, trim uses an EXPLICIT, ABSOLUTE start
        // (matching the source's own original timeline) rather than
        // relying on any outer "-ss" to position things first — and the
        // outer "-ss"/"-t" flags are omitted entirely below for this
        // case. Two earlier attempts at combining an outer seek with an
        // internal trim/setpts chain both showed real, confirmed bugs
        // (wrong source range selected; then a segment's reported
        // duration coming back as nearly the entire rest of the source
        // file), which points at the interaction between outer seeking
        // and this filter chain being unreliable in ways not worth
        // continuing to guess at. Removing the outer seek entirely
        // avoids that interaction as a category: `trim` here reads the
        // source's raw, original timestamps with nothing else having
        // touched them first, so its behavior is unambiguous. Slower —
        // ffmpeg decodes from the file's start to reach this section
        // rather than jumping ahead — but correctness matters more than
        // speed for what's already a less common case (most exports
        // never touch speed at all).
        if speed_active {
            filters.push(format!("trim=start={}:duration={seg_duration}", section.in_time));
        }
        if let Some(f) = crop_filter(params.crop_x, params.crop_y, params.crop_width, params.crop_height) {
            filters.push(f);
        }
        // Set (only when burn_subs is active) to this section's own
        // subtitle temp folder, so subtitle_filter()'s bare filename
        // resolves correctly — see subtitle_filter's own comment for
        // the full history of why a bare filename + matching cwd is
        // used here instead of embedding a path directly.
        let mut cwd: Option<std::path::PathBuf> = None;
        if params.burn_subs {
            let temp_dir = std::env::temp_dir().join(format!("klippit-{temp_prefix}-subs{i}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&temp_dir);
            // Rebased to THIS section's own start — critical for
            // multi-section exports: a section starting at, say, 1:00 in
            // the source needs its subtitle timestamps shifted back to
            // start at 0, since in the combined output that content
            // plays starting wherever this section lands after concat,
            // not at the source's original 1:00 mark.
            let extracted = extract_subtitles_to(
                app, &params.file_path, &temp_dir, Some((section.in_time, seg_duration)),
                params.subtitle_external_file.as_deref(), params.subtitle_lang.as_deref(),
            ).await?;
            filters.push(subtitle_filter(&extracted, effective_width, effective_height));
            cwd = Some(temp_dir);
        }
        // Speed change goes LAST in the video filter chain, after crop
        // and subtitle burn-in — subtitles get burned in at their
        // normal, correct timing first, then setpts speeds up the whole
        // resulting frame (video plus now-burned-in text) together. This
        // avoids needing any separate subtitle-timestamp scaling for
        // speed at all: the text is just pixels in the frame by the
        // time setpts touches anything.
        //
        // (PTS-STARTPTS), not just PTS: trim above leaves this stream's
        // timestamps starting at section.in_time (its own absolute
        // source position, not rebased to zero — trim only filters which
        // frames pass through, it doesn't renumber them), so this single
        // expression both rebases to zero AND applies the speed change
        // in one step.
        let mut audio_filters: Vec<String> = vec![];
        if speed_active {
            filters.push(format!("setpts=(PTS-STARTPTS)/{:.6}", section.speed));
            if keep_audio {
                // Same reasoning as the video side, mirrored for audio:
                // atrim with the same explicit absolute start first
                // (raw, unambiguous input-time limit), asetpts to
                // rebase, then the actual tempo change.
                audio_filters.push(format!("atrim=start={}:duration={seg_duration}", section.in_time));
                audio_filters.push("asetpts=PTS-STARTPTS".into());
                audio_filters.push(atempo_chain(section.speed));
            }
        }

        // Two different seeking strategies depending on whether speed is
        // active, not one shared path:
        //
        // - Normal speed (the common case): outer "-ss" placed AFTER
        //   "-i" (output/accurate seeking, not the fast input-seeking
        //   used elsewhere in this file) plus outer "-t seg_duration".
        //   Slower than input-seeking but accurate — multi-section glues
        //   several of these together, so any per-segment seek
        //   imprecision becomes a visible seam exactly at a join. No
        //   filter-chain interaction to worry about here since there's
        //   no setpts involved at all.
        //
        // - Speed active: NO outer "-ss"/"-t" at all — trim (added to
        //   the filter chain above) handles both start and duration
        //   explicitly instead, on the source's raw, untouched
        //   timestamps. This was arrived at after two earlier attempts
        //   at combining an outer seek with trim/setpts both showed
        //   real, confirmed bugs (wrong source range selected; then a
        //   segment's reported duration coming back as nearly the
        //   entire rest of the source file) — removing the outer seek
        //   entirely avoids that interaction as a category rather than
        //   continuing to guess at exactly how it was going wrong.
        //   Slower (decodes from the file's start every time) but
        //   unambiguous.
        let mut extract_args: Vec<String> = vec!["-y".into(), "-i".into(), params.file_path.clone()];
        if !speed_active {
            extract_args.push("-ss".into());
            extract_args.push(section.in_time.to_string());
            extract_args.push("-t".into());
            extract_args.push(seg_duration.to_string());
        }
        if !filters.is_empty() {
            extract_args.push("-vf".into());
            extract_args.push(filters.join(","));
        }
        extract_args.push("-c:v".into());
        extract_args.push("libx264".into());
        extract_args.push("-crf".into());
        extract_args.push("18".into()); // visually near-lossless — every intermediate segment is a throwaway, not the final output
        extract_args.push("-preset".into());
        extract_args.push("veryfast".into());
        if keep_audio {
            // Every segment re-encoded with identical audio settings —
            // required for the final concat's stream-copy to work
            // correctly across segments.
            extract_args.push("-c:a".into());
            extract_args.push("aac".into());
            extract_args.push("-b:a".into());
            extract_args.push("192k".into());
            if !audio_filters.is_empty() {
                extract_args.push("-af".into());
                extract_args.push(audio_filters.join(","));
            }
        } else {
            extract_args.push("-an".into());
        }
        // Forces this segment's own timestamps to cleanly rebase to
        // zero. Without this, "-ss before -i" seeking can leave small
        // timestamp irregularities (especially around B-frames near the
        // seek point) that don't reset cleanly — harmless within a
        // single standalone segment, but the concat step below only
        // stream-copies raw packets and timestamps (-c copy, chosen for
        // speed since every segment is already freshly re-encoded here),
        // so it has no opportunity to fix up a segment that didn't start
        // clean. That's the likely cause of a real reported symptom:
        // a stutter/gap exactly at a section boundary, with the
        // following section's visible playtime feeling short even
        // though the file's total reported duration matches.
        extract_args.push("-avoid_negative_ts".into());
        extract_args.push("make_zero".into());
        extract_args.push(seg_path.to_string_lossy().to_string());
        run_bin(app, "ffmpeg", &extract_args, cwd.as_deref()).await?;
        segment_paths.push(seg_path);
    }

    if segment_paths.len() == 1 {
        return Ok(segment_paths.into_iter().next().unwrap());
    }

    // Concat FILTER (-filter_complex ... concat=...), not the concat
    // DEMUXER (-f concat -c copy) used here previously. This is a
    // deliberate escalation, not a first attempt: two prior fixes at the
    // demuxer/stream-copy level (-avoid_negative_ts make_zero on both
    // the per-segment extraction and the demuxer step, then switching
    // segment extraction to accurate output-seeking) were both
    // confirmed present in the actual commands that ran, and neither
    // resolved a real reported stutter/gap at segment boundaries. The
    // concat demuxer's `-c copy` only splices compressed packets and
    // trusts the segments are perfectly compatible — it has no
    // mechanism to correct a residual per-segment quirk (encoder
    // priming delay, any leftover timing irregularity), it just
    // faithfully preserves whatever's there straight into the output.
    // The concat filter instead operates on fully DECODED frames: every
    // segment gets decoded and the filter itself re-times everything
    // into one continuous sequence, which eliminates packet/timestamp-
    // level splice concerns as a category rather than continuing to
    // patch around them. Costs a real re-encode at this step (previously
    // just a fast stream copy), acceptable here since correctness at a
    // segment boundary matters more than shaving time off an
    // intermediate step for what are typically short clips anyway.
    let mut concat_args: Vec<String> = vec!["-y".into()];
    for p in &segment_paths {
        // Generous probe budget for each input — the error this fixes
        // ("Could not find codec parameters... unspecified pixel
        // format", with ffmpeg's own suggestion to raise these) showed
        // up specifically for a segment that had gone through setpts
        // (a speed change): rewriting timestamps there leaves the file
        // with less standard-looking packet timing that the concat
        // filter's default, tighter probe budget couldn't reliably read
        // — worse still with several inputs being probed at once here.
        // analyzeduration is in MICROSECONDS, probesize in BYTES — easy
        // to mix up; values below are generously oversized relative to
        // these small, few-second intermediate segment files, so this
        // costs nothing when probing succeeds quickly anyway.
        concat_args.push("-analyzeduration".into());
        concat_args.push("100000000".into());
        concat_args.push("-probesize".into());
        concat_args.push("50000000".into());
        concat_args.push("-i".into());
        concat_args.push(p.to_string_lossy().to_string());
    }
    let n = segment_paths.len();
    let mut filter = String::new();
    for i in 0..n {
        filter.push_str(&format!("[{i}:v]"));
        if keep_audio { filter.push_str(&format!("[{i}:a]")); }
    }
    let a_flag = if keep_audio { 1 } else { 0 };
    filter.push_str(&format!("concat=n={n}:v=1:a={a_flag}[outv]"));
    if keep_audio { filter.push_str("[outa]"); }
    concat_args.push("-filter_complex".into());
    concat_args.push(filter);
    concat_args.push("-map".into());
    concat_args.push("[outv]".into());
    if keep_audio {
        concat_args.push("-map".into());
        concat_args.push("[outa]".into());
    }
    concat_args.push("-c:v".into());
    concat_args.push("libx264".into());
    concat_args.push("-crf".into());
    concat_args.push("18".into()); // still an intermediate, throwaway file — the final format-specific step re-encodes again
    concat_args.push("-preset".into());
    concat_args.push("veryfast".into());
    if keep_audio {
        concat_args.push("-c:a".into());
        concat_args.push("aac".into());
        concat_args.push("-b:a".into());
        concat_args.push("192k".into());
    } else {
        concat_args.push("-an".into());
    }

    let combined_path = std::env::temp_dir().join(format!("klippit-{temp_prefix}-combined-{}.mkv", std::process::id()));
    concat_args.push(combined_path.to_string_lossy().to_string());
    let concat_result = run_bin(app, "ffmpeg", &concat_args, None).await;

    // Best-effort cleanup of the per-section intermediates regardless of
    // outcome — only the combined file (or the error) matters past this
    // point.
    for p in &segment_paths { let _ = std::fs::remove_file(p); }

    concat_result?;
    Ok(combined_path)
}



fn scale_filter(resolution: u32) -> Option<String> {
    if resolution == 0 { return None; }
    // Scale to target height, preserve aspect ratio, even dimensions
    // (required by most encoders).
    Some(format!("scale=-2:{resolution}"))
}

// Static crop applied uniformly to the whole clip. All four fields are
// checked together — callers only ever pass all-Some or all-None, but
// this stays defensive against a partial set rather than assuming that
// invariant holds. Takes raw values rather than &ExportParams so it's
// reusable from extract_frame (screenshots), which has its own
// individual parameters rather than an ExportParams instance.
// atempo (audio tempo/speed change, pitch-preserving) only accepts
// 0.5–2.0 per filter instance — chaining multiple instances covers the
// full 0.25x–4x range this app exposes (e.g. 4.0 becomes two chained
// atempo=2.0 filters, since 2.0 * 2.0 = 4.0).
fn atempo_chain(speed: f64) -> String {
    let mut factors: Vec<f64> = vec![];
    let mut remaining = speed;
    if remaining < 0.5 {
        while remaining < 0.5 {
            factors.push(0.5);
            remaining /= 0.5;
        }
        factors.push(remaining);
    } else if remaining > 2.0 {
        while remaining > 2.0 {
            factors.push(2.0);
            remaining /= 2.0;
        }
        factors.push(remaining);
    } else {
        factors.push(remaining);
    }
    factors.iter().map(|f| format!("atempo={f:.6}")).collect::<Vec<_>>().join(",")
}

fn crop_filter(x: Option<u32>, y: Option<u32>, w: Option<u32>, h: Option<u32>) -> Option<String> {
    match (x, y, w, h) {
        (Some(x), Some(y), Some(w), Some(h)) if w > 0 && h > 0 => {
            Some(format!("crop={w}:{h}:{x}:{y}"))
        }
        _ => None,
    }
}

// Builds the actual `subtitles=...` filter string from already-extracted,
// clean-path subtitle/font files (see extract_subtitles_to) — never from
// the original source path directly. That's the fix for a real bug: a
// real-world filename like "[SubsPlease] Show - 09 [ABCD1234].mkv"
// contains brackets that corrupted ffmpeg's filter-option parsing when
// pointed at directly, producing a garbled "unable to parse
// original_size" error that had nothing to do with original_size
// itself — parsing had already gone wrong on the bracket-heavy path
// before reaching that option.
//
// Timing correctness note: extraction preserves the subtitle stream's
// original absolute timestamps (no -ss applied during extraction), which
// is what keeps this correctly in sync with a trimmed, -ss-shifted
// output — the filter matches its subtitle file's absolute timestamps
// against frame timing in the graph the same way it would reading
// straight from the original source, just via a cleanly-named
// intermediate file instead of the messy original path.
fn subtitle_filter(extracted: &ExtractedSubtitles, source_width: u32, source_height: u32) -> String {
    // Fourth attempt at the Windows drive-letter-colon problem in
    // ffmpeg's subtitles filter, and this one has actual evidence behind
    // it rather than another guess. Real ffmpeg 9.0.2 output confirmed
    // the third attempt's backslash-escaped colon (filename=C\:/...) was
    // NOT treated as a literal colon at all: the parser still split
    // there ("No option name near '/Users/...'" — it consumed "C" as
    // the complete value and choked on the remainder). That's now three
    // separate escaping attempts across this debugging history that
    // have all failed the same way, which is a strong signal that
    // escaping this colon directly isn't the right mechanism for this
    // filter's parser, whatever the exact reason.
    //
    // Reverting to bare filename + a matching process working directory
    // instead — avoids the colon question entirely rather than
    // continuing to guess at escape syntax. This was abandoned earlier
    // based on a vague "still doesn't work" report, but that report
    // came before the actual confirmed bug (run_bin's stdout
    // newline-accumulation issue, breaking subtitle STREAM DETECTION
    // entirely) was found and fixed — meaning no subtitles were ever
    // being extracted at all at that point, regardless of whether cwd
    // worked. This approach never actually had contrary evidence
    // against it; it just never got a fair test. See extract_and_
    // concat_sections and extract_frame for where cwd is set back to
    // match this section's/screenshot's own extraction temp folder.
    let filename = extracted.ass_path.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "subs.ass".to_string());
    // fontsdir intentionally still dropped: it was implicated in the
    // same class of error and hasn't been re-verified since. Tradeoff:
    // burned-in text may render in a system fallback font instead of
    // the exact embedded one if that font isn't already installed.
    let mut filter = format!("subtitles=filename={filename}");
    // Explicitly telling the filter the source's real resolution avoids a
    // separate real bug hit in testing: ffmpeg's auto-detection of this
    // can fail ("Unable to parse 'original_size' option value '0x0'")
    // depending on where in the filter chain subtitles sits.
    if source_width > 0 && source_height > 0 {
        filter.push_str(&format!(":original_size={source_width}x{source_height}"));
    }
    filter
}

async fn export_mp4(app: &AppHandle, params: &ExportParams, duration: f64, out_path: &str, effective_width: u32, effective_height: u32) -> Result<String, String> {
    let combined_path = extract_and_concat_sections(app, params, effective_width, effective_height, !params.mute_audio, "mp4").await?;
    let combined_path_str = combined_path.to_string_lossy().to_string();

    // Crop and subtitles are already baked into combined_path (applied
    // per-section during extraction) — only scale remains to apply
    // here, last of all, on the already-assembled, already-burned-in
    // frame. combined_path is also already exactly the desired content
    // start to finish, so no -ss/-t trimming is needed reading from it
    // (start=0.0, full duration) the way the old single-section version
    // needed to seek into the original source directly.
    let vf = scale_filter(params.resolution);

    let result = if params.mode == "size" {
        // Target-size mode: compute bitrate from target_size / duration,
        // reserve a fixed slice for audio, 2-pass encode to hit it
        // reliably. Muted: skip that reservation entirely and give the
        // whole bitrate budget to video instead. GPU encoding isn't
        // offered here — see the note on use_gpu in ExportParams.
        let target_bits = params.target_mb * 8_000_000.0;
        let audio_kbps = if params.mute_audio { 0.0 } else { 128.0 };
        let video_kbps = ((target_bits / duration / 1000.0) - audio_kbps).max(200.0);

        run_ffmpeg_2pass(app, &combined_path_str, 0.0, duration, vf.as_deref(), video_kbps, audio_kbps, out_path, params.mute_audio).await
            .map(|_| "libx264".to_string())
    } else {
        encode_quality_mode(app, params, &combined_path_str, 0.0, duration, vf.as_deref(), out_path).await
    };

    let _ = std::fs::remove_file(&combined_path);
    result
}

// GPU-accelerated encoding: opt-in (params.use_gpu), with automatic,
// silent fallback to the regular software encoder (libx264) if a
// hardware encoder isn't available or fails for any reason. Tried in
// this order since NVIDIA GPUs are the most common consumer option for
// this kind of workload, then AMD, then Intel QuickSync — but nothing
// here actually detects which GPU vendor is present; it simply attempts
// each encoder in turn and moves on immediately if ffmpeg reports it
// can't be used, which naturally handles "wrong/no GPU vendor" without
// needing to know that in advance. Every attempt uses the exact same
// input, filters, and audio handling as the software path — only the
// video codec and its quality option change.
//
// UNVERIFIED: the exact quality-parameter syntax for each hardware
// encoder (-cq for nvenc, -qp_i/-qp_p for amf, -global_quality for qsv)
// is written from ffmpeg's documented options for each, not confirmed
// against a real GPU of each vendor — there's no way to test that here.
// If a particular encoder's quality parameter turns out wrong, the
// worst case is that specific attempt fails and this same fallback
// logic moves on to the next option (or to libx264) exactly as it would
// for "no such GPU" — this can't produce a broken export, only
// possibly a less-than-ideal encoder choice for that one attempt.
const GPU_ENCODER_NAMES: &[&str] = &["h264_nvenc", "h264_amf", "h264_qsv"];

fn gpu_quality_args(encoder: &str, crf: u32) -> Vec<String> {
    match encoder {
        "h264_nvenc" => vec!["-rc".into(), "vbr".into(), "-cq".into(), crf.to_string()],
        "h264_amf" => vec!["-rc".into(), "cqp".into(), "-qp_i".into(), crf.to_string(), "-qp_p".into(), crf.to_string()],
        "h264_qsv" => vec!["-global_quality".into(), crf.to_string()],
        _ => vec![],
    }
}

async fn encode_quality_mode(
    app: &AppHandle, params: &ExportParams, input: &str, start: f64, duration: f64, vf: Option<&str>,
    out_path: &str,
) -> Result<String, String> {
    let mut audio_args: Vec<String> = vec![];
    if params.mute_audio {
        audio_args.push("-an".into());
    } else {
        audio_args.push("-c:a".into()); audio_args.push("aac".into());
        audio_args.push("-b:a".into()); audio_args.push("128k".into());
    }

    if params.use_gpu {
        for encoder in GPU_ENCODER_NAMES {
            let mut args: Vec<String> = vec![
                "-y".into(),
                "-ss".into(), start.to_string(),
                "-i".into(), input.to_string(),
                "-t".into(), duration.to_string(),
                "-c:v".into(), (*encoder).to_string(),
            ];
            args.extend(gpu_quality_args(encoder, params.crf));
            args.extend(audio_args.clone());
            if let Some(f) = vf { args.push("-vf".into()); args.push(f.to_string()); }
            args.push(out_path.into());
            if run_bin(app, "ffmpeg", &args, None).await.is_ok() {
                return Ok((*encoder).to_string());
            }
            // This specific hardware encoder isn't available or failed
            // for some other reason — move on to the next one, or to
            // libx264 below, exactly as if no GPU were present at all.
        }
    }

    // Software fallback — either GPU wasn't requested, or every
    // hardware option above failed.
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-ss".into(), start.to_string(),
        "-i".into(), input.to_string(),
        "-t".into(), duration.to_string(),
        "-crf".into(), params.crf.to_string(),
        "-preset".into(), "medium".into(),
    ];
    args.extend(audio_args);
    if let Some(f) = vf { args.push("-vf".into()); args.push(f.to_string()); }
    args.push(out_path.into());
    run_bin(app, "ffmpeg", &args, None).await?;
    Ok("libx264".to_string())
}

async fn run_ffmpeg_2pass(
    app: &AppHandle, input: &str, start: f64, duration: f64, vf: Option<&str>,
    video_kbps: f64, audio_kbps: f64, out_path: &str,
    mute_audio: bool,
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
    run_bin(app, "ffmpeg", &pass1, None).await?;

    let mut pass2: Vec<String> = vec![
        "-y".into(), "-ss".into(), start.to_string(), "-i".into(), input.into(),
        "-t".into(), duration.to_string(),
        "-c:v".into(), "libx264".into(), "-b:v".into(), bitrate,
        "-pass".into(), "2".into(), "-passlogfile".into(), passlog_prefix.clone(),
    ];
    if mute_audio {
        pass2.push("-an".into());
    } else {
        pass2.push("-c:a".into()); pass2.push("aac".into());
        pass2.push("-b:a".into()); pass2.push(audio);
    }
    if let Some(f) = vf { pass2.push("-vf".into()); pass2.push(f.into()); }
    pass2.push(out_path.into());
    let result = run_bin(app, "ffmpeg", &pass2, None).await.map(|_| ());

    // Best-effort cleanup — leftover pass-log files in the temp dir are
    // harmless either way, so a failed removal here doesn't fail the export.
    let _ = std::fs::remove_file(format!("{passlog_prefix}-0.log"));
    let _ = std::fs::remove_file(format!("{passlog_prefix}-0.log.mbtree"));

    result
}

async fn export_gif(app: &AppHandle, params: &ExportParams, out_path: &str, effective_width: u32, effective_height: u32) -> Result<(), String> {
    // Extraction (each section, crop/subtitles baked in per-section, then
    // concatenated if there's more than one) happens once here, rather
    // than re-seeking into the original source for every single
    // palette-gen/paletteuse pass and every target-size retry attempt.
    // Confirmed via real testing: a clip ~9 minutes into a file took
    // ~6 minutes to GIF-export even with duration correctly bounded —
    // almost certainly the cost of two separate deep seeks into the
    // source (one per pass), which in target-size mode could happen up
    // to six times across retries. Extracting once up front pays that
    // cost exactly once no matter how many GIF passes follow, since
    // every subsequent pass reads from this small file starting at
    // position 0 — no seeking needed there at all.
    let combined_path = extract_and_concat_sections(app, params, effective_width, effective_height, false, "gif").await?;
    let segment_path = combined_path.to_string_lossy().to_string();

    // Two-pass palette gen/use — same technique as the wasm version, just
    // real ffmpeg instead of a wasm build. Target-size mode iterates width/
    // fps rather than a bitrate knob, since GIF has none.
    let (mut width, mut fps) = if params.mode == "size" {
        (480u32, params.gif_fps.max(10))
    } else {
        (if params.resolution == 0 { 480 } else { params.resolution }, params.gif_fps)
    };

    let mut result = Ok(());
    for attempt in 0..3 {
        result = encode_gif_attempt(app, &segment_path, width, fps, out_path).await;
        if result.is_err() || params.mode != "size" { break; }

        let size_mb = std::fs::metadata(out_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
        if size_mb <= params.target_mb || attempt == 2 { break; }
        // Overshot — back off width and fps and retry once or twice.
        width = (width as f64 * 0.8) as u32;
        fps = (fps as f64 * 0.85).max(8.0) as u32;
    }

    // Best-effort cleanup — a leftover temp segment is harmless either
    // way, so a failed removal here doesn't fail the export.
    let _ = std::fs::remove_file(&combined_path);
    result
}

async fn encode_gif_attempt(app: &AppHandle, segment_path: &str, width: u32, fps: u32, out_path: &str) -> Result<(), String> {
    // No seeking, no subtitle filter, no cwd needed here anymore — all
    // of that already happened once in export_gif's upfront extraction.
    // segment_path is a small, already-trimmed, already-subtitled file
    // starting at position 0, so every pass here just reads straight
    // through it start to finish.
    let base = format!("fps={fps},scale={width}:-2:flags=lanczos");
    let palette = format!("{out_path}.palette.png");

    run_bin(app, "ffmpeg", &[
        "-y".into(), "-i".into(), segment_path.to_string(),
        "-vf".into(), format!("{base},palettegen"),
        palette.clone(),
    ], None).await?;

    run_bin(app, "ffmpeg", &[
        "-y".into(), "-i".into(), segment_path.to_string(),
        "-i".into(), palette.clone(),
        "-lavfi".into(), format!("{base}[x];[x][1:v]paletteuse=dither=bayer"),
        out_path.into(),
    ], None).await?;

    let _ = std::fs::remove_file(&palette);
    Ok(())
}

async fn export_apng(app: &AppHandle, params: &ExportParams, out_path: &str, effective_width: u32, effective_height: u32) -> Result<(), String> {
    // Same extract-once (per section, then concatenated if more than
    // one) approach as GIF and for the same reason, but simpler: APNG
    // needs no palette generation at all, so a retry here is just one
    // direct ffmpeg call against the already-extracted segment, not
    // GIF's two-pass palettegen/paletteuse dance.
    let combined_path = extract_and_concat_sections(app, params, effective_width, effective_height, false, "apng").await?;
    let segment_path = combined_path.to_string_lossy().to_string();

    // APNG is lossless — there's no CRF-equivalent quality knob the way
    // MP4 has, so resolution and fps are the only real file-size levers,
    // the same two GIF already exposes. Target-size mode reuses that
    // identical shrink-and-retry approach.
    let (mut width, mut fps) = if params.mode == "size" {
        (480u32, params.gif_fps.max(10))
    } else {
        (if params.resolution == 0 { 480 } else { params.resolution }, params.gif_fps)
    };

    let mut result = Ok(());
    for attempt in 0..3 {
        result = encode_apng_attempt(app, &segment_path, width, fps, out_path).await;
        if result.is_err() || params.mode != "size" { break; }

        let size_mb = std::fs::metadata(out_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
        if size_mb <= params.target_mb || attempt == 2 { break; }
        width = (width as f64 * 0.8) as u32;
        fps = (fps as f64 * 0.85).max(8.0) as u32;
    }

    let _ = std::fs::remove_file(&combined_path);
    result
}

async fn encode_apng_attempt(app: &AppHandle, segment_path: &str, width: u32, fps: u32, out_path: &str) -> Result<(), String> {
    // -plays 0 loops forever, matching GIF's default looping behavior.
    // No palette step at all, unlike GIF — PNG doesn't have an 8-bit/
    // 256-color ceiling, so this is one direct pass.
    run_bin(app, "ffmpeg", &[
        "-y".into(), "-i".into(), segment_path.to_string(),
        "-vf".into(), format!("fps={fps},scale={width}:-2:flags=lanczos"),
        "-plays".into(), "0".into(),
        out_path.into(),
    ], None).await?;
    Ok(())
}

// ---------- screenshots ----------
// Screenshots are handled by mpv itself (`screenshot video` / `screenshot
// subtitles`) — see mpv-scripts/clip-trigger.lua. This command is a
// fallback for extracting a still via ffmpeg if a screenshot is ever
// requested outside of an active mpv session.
#[tauri::command]
async fn extract_frame(app: AppHandle, path: String, at: f64, burn_subs: bool, out_path: String, source_width: u32, source_height: u32, subtitle_lang: Option<String>, subtitle_external_file: Option<String>, crop_x: Option<u32>, crop_y: Option<u32>, crop_width: Option<u32>, crop_height: Option<u32>) -> Result<String, String> {
    let out_path = expand_tilde(&out_path);
    if let Some(parent) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("couldn't create output folder: {e}"))?;
    }
    let mut args: Vec<String> = vec![
        "-y".into(), "-ss".into(), at.to_string(), "-i".into(), path.clone(),
        "-frames:v".into(), "1".into(),
    ];
    // Crop applies here too — a screenshot should match whatever the
    // live preview is currently showing, and crop is a real, visible
    // editing decision the same way subtitles-on/off already is.
    let crop = crop_filter(crop_x, crop_y, crop_width, crop_height);
    // original_size (if burning subtitles) uses the cropped dimensions
    // when crop is active, same reasoning as the export path: subtitles
    // are burned onto the already-cropped frame, so they need to be
    // sized relative to that, not the pre-crop source.
    let (effective_width, effective_height) = match (crop_width, crop_height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => (source_width, source_height),
    };
    let mut cwd: Option<std::path::PathBuf> = None;
    if burn_subs {
        // Bare filename + matching cwd — see subtitle_filter's own
        // comment for the full history of why (three escaping attempts
        // at embedding the full path directly all failed against real
        // ffmpeg builds).
        let temp_dir = std::env::temp_dir().join(format!("klippit-shot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let extracted = extract_subtitles_to(
            &app, &path, &temp_dir, Some((at, 1.0)),
            subtitle_external_file.as_deref(), subtitle_lang.as_deref(),
        ).await?;
        let mut shot_filters: Vec<String> = vec![];
        if let Some(f) = &crop { shot_filters.push(f.clone()); }
        shot_filters.push(subtitle_filter(&extracted, effective_width, effective_height));
        args.push("-vf".into());
        args.push(shot_filters.join(","));
        cwd = Some(temp_dir);
    } else if let Some(f) = &crop {
        args.push("-vf".into());
        args.push(f.clone());
    }
    args.push(out_path.clone());
    run_bin(&app, "ffmpeg", &args, cwd.as_deref()).await?;
    Ok(out_path)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubtitlePreviewData {
    ass_path: String,
    font_paths: Vec<String>,
}

// Shared by both the preview overlay and export burn-in: extracts the
// first subtitle stream (converted to ASS regardless of source codec —
// SRT, SSA, whatever) plus embedded font attachments, all into a clean
// temp directory with safe, predictable filenames — deliberately NOT the
// original file's own name, which is a real bug source: a real-world
// filename like "[SubsPlease] Show - 09 (720p) [ABCD1234].mkv" contains
// brackets that corrupted ffmpeg's filter-option parsing when the
// subtitles filter pointed directly at it, producing garbled downstream
// errors (a mangled "original_size" value) that had nothing to do with
// original_size itself — the parser had already gone off the rails on
// the bracket-heavy path before it got there. Pointing at a clean
// extracted path sidesteps the whole class of problem regardless of how
// messy the source filename is.
//
// Absolute subtitle timestamps are preserved (no -ss during extraction),
// matching the same assumption documented on the old direct-file
// approach: burning in against a file whose subtitle timestamps still
// match the original absolute timeline stays correctly synced with a
// trimmed, -ss-shifted output.
//
// STATUS: the attachment-dumping command construction here (multiple
// -dump_attachment:INDEX flags batched into one ffmpeg call) is written
// from documented ffmpeg wiki patterns, not yet confirmed against a real
// run — if font extraction silently returns nothing, that command is the
// first place to check by running it manually in a terminal.
struct ExtractedSubtitles {
    ass_path: std::path::PathBuf,
    font_paths: Vec<std::path::PathBuf>,
}

// `trim`: Some((in_time, duration)) for export/screenshot use — extracts
// only that window, with output-side -ss/-t (subtitle streams are tiny,
// so accuracy costs nothing here) so the extracted file's own timestamps
// rebase to start near zero exactly like the main video's frames do
// under the encode step's input-side -ss. Without this, a real timing
// bug showed up: burned-in subtitles were completely out of sync,
// because the subtitle filter was comparing the video's rebased-to-zero
// frame times against the subtitle file's original absolute timestamps.
// None for the live preview overlay, which shows the whole file, not a
// trimmed segment, so no rebasing is needed or wanted there.
// `trim`: Some((in_time, duration)) for export/screenshot use — extracts
// only that window, with output-side -ss/-t (subtitle streams are tiny,
// so accuracy costs nothing here) so the extracted file's own timestamps
// rebase to start near zero exactly like the main video's frames do
// under the encode step's input-side -ss. Without this, a real timing
// bug showed up: burned-in subtitles were completely out of sync,
// because the subtitle filter was comparing the video's rebased-to-zero
// frame times against the subtitle file's original absolute timestamps.
// None for the live preview overlay, which shows the whole file, not a
// trimmed segment, so no rebasing is needed or wanted there.
//
// `external_file`: if the player has an externally-loaded subtitle file
// active (not embedded in the video container — e.g. VLC's "Add
// Subtitle File", or mpv playing alongside a same-named .srt), this
// takes priority and gets used directly, since it's genuinely what's
// being watched. Passed through as Some(path) only when non-empty.
//
// `preferred_lang`: when there's no external file, used to pick the
// right EMBEDDED stream out of possibly several (a file with English +
// Spanish + French tracks, say) by matching ffprobe's own
// stream_tags=language against the player's reported active-track
// language — far more robust than trying to map mpv's or VLC's internal
// track numbering onto ffprobe's container stream indices, which aren't
// guaranteed to correspond 1:1. Falls back to the first subtitle stream
// found if there's no language match (or no language info available at
// all) — better than failing outright, though it's the same "might
// silently pick the wrong track" behavior this whole feature exists to
// fix, just as a last resort rather than the default.
// Parses an .ass timestamp (H:MM:SS.CC — hours with no leading zero,
// then minutes:seconds.centiseconds) into total seconds.
fn parse_ass_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 3 { return None; }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn format_ass_time(total_seconds: f64) -> String {
    let total_seconds = total_seconds.max(0.0);
    let h = (total_seconds / 3600.0).floor() as u64;
    let m = ((total_seconds % 3600.0) / 60.0).floor() as u64;
    let sec = total_seconds % 60.0;
    format!("{h}:{m:02}:{sec:05.2}")
}

// Shifts every Dialogue line's Start/End timestamps by -offset_seconds,
// rewriting the file in place. Necessary because ffmpeg's own "-ss"/"-t"
// output-seeking trim does NOT rebase an .ass subtitle stream's own
// embedded Dialogue timestamps to start at 0 the way it does for
// video/audio packet timestamps — .ass stores each event's timing as
// literal text within the line itself (Dialogue: 0,0:01:46.05,...), not
// as container-level packet timestamps, so ffmpeg's trim only filters
// which lines make it into the output without rewriting their text.
// Confirmed as the actual cause of a real "export succeeds, no subtitles
// appear" bug via real ffmpeg command logs: the extracted .ass file kept
// its ORIGINAL, un-shifted times (e.g. still ~106s into the source),
// while the video segment it was burned onto had already been rebased
// to start at 0 and was only ~5.7s long — the subtitle timing pointed at
// a moment in the segment's non-existent future and simply never arrived.
fn shift_ass_timestamps(ass_path: &std::path::Path, offset_seconds: f64, duration: f64) -> Result<(), String> {
    let content = std::fs::read_to_string(ass_path).map_err(|e| e.to_string())?;
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Dialogue:") {
            // Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text —
            // splitn(10, ',') keeps the Text field intact even if it
            // contains commas of its own, since only the first 9 are
            // ever structural.
            let fields: Vec<&str> = rest.splitn(10, ',').collect();
            if fields.len() == 10 {
                if let (Some(s), Some(e)) = (parse_ass_time(fields[1]), parse_ass_time(fields[2])) {
                    // Drop lines entirely outside [offset_seconds,
                    // offset_seconds + duration] rather than shifting
                    // them — a real, confirmed bug: a line that ends
                    // before the clip's own start would shift to a
                    // NEGATIVE timestamp, which format_ass_time's
                    // .max(0.0) clamp then squashed to exactly 0 instead
                    // of excluding it, making dialogue from well before
                    // the marked In point incorrectly appear at the very
                    // start of the export — confirmed via a side-by-side
                    // comparison showing the wrong line burned in at
                    // frame 0 versus what the source actually shows at
                    // that timestamp. Same reasoning applies to a line
                    // starting after the clip ends.
                    if e <= offset_seconds || s >= offset_seconds + duration {
                        continue;
                    }
                    out.push_str("Dialogue:");
                    out.push_str(fields[0]);
                    out.push(',');
                    out.push_str(&format_ass_time(s - offset_seconds));
                    out.push(',');
                    out.push_str(&format_ass_time(e - offset_seconds));
                    for f in &fields[3..] {
                        out.push(',');
                        out.push_str(f);
                    }
                    out.push('\n');
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    std::fs::write(ass_path, out).map_err(|e| e.to_string())
}

async fn extract_subtitles_to(
    app: &AppHandle, path: &str, temp_dir: &std::path::Path, trim: Option<(f64, f64)>,
    external_file: Option<&str>, preferred_lang: Option<&str>,
) -> Result<ExtractedSubtitles, String> {
    std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;
    let ass_path = temp_dir.join("subs.ass");

    // External file takes priority — it's a separate, standalone
    // subtitle file, not a stream within the video container at all, so
    // there's no "-map 0:N" involved: just convert it directly. Extracts
    // the FULL, untrimmed file (no -ss/-t here — see shift_ass_timestamps'
    // own comment for why ffmpeg's trim can't be trusted to rebase an
    // .ass file's embedded timestamps), then shifts timestamps manually
    // afterward if a trim was requested.
    if let Some(ext) = external_file.filter(|s| !s.is_empty()) {
        let extract_args: Vec<String> = vec![
            "-y".into(), "-i".into(), ext.to_string(),
            "-c:s".into(), "ass".into(),
            ass_path.to_string_lossy().to_string(),
        ];
        run_bin(app, "ffmpeg", &extract_args, None).await?;
        if let Some((in_time, duration)) = trim {
            shift_ass_timestamps(&ass_path, in_time, duration)?;
        }
        // External files don't carry embedded font attachments the way
        // an MKV might — nothing to extract there.
        return Ok(ExtractedSubtitles { ass_path, font_paths: vec![] });
    }

    // No external file — search the container's own subtitle streams,
    // preferring one whose language tag matches what the player reported
    // as active, falling back to the first one found.
    let stream_probe = run_bin(app, "ffprobe", &[
        "-v".into(), "error".into(),
        "-select_streams".into(), "s".into(),
        "-show_entries".into(), "stream=index:stream_tags=language".into(),
        "-of".into(), "csv=p=0".into(),
        path.to_string(),
    ], None).await?;

    let mut matched_index: Option<u32> = None;
    let mut first_index: Option<u32> = None;
    for line in String::from_utf8_lossy(&stream_probe).lines() {
        let mut parts = line.splitn(2, ',');
        let idx = parts.next().and_then(|s| s.trim().parse::<u32>().ok());
        let Some(idx) = idx else { continue };
        if first_index.is_none() { first_index = Some(idx); }
        if let Some(pref) = preferred_lang {
            let lang = parts.next().unwrap_or("").trim();
            if !lang.is_empty() {
                // Exact match first (this is what mpv's clean ISO codes
                // like "eng" always hit), then a lenient substring check
                // as a fallback — VLC's own track-language reporting is
                // more likely to be a human-readable name ("English")
                // than a clean ISO code, so this gives it a real chance
                // to still match ffprobe's "eng" tag either direction.
                let lang_lower = lang.to_ascii_lowercase();
                let pref_lower = pref.to_ascii_lowercase();
                if lang_lower == pref_lower
                    || pref_lower.contains(&lang_lower)
                    || lang_lower.contains(&pref_lower)
                {
                    matched_index = Some(idx);
                    break;
                }
            }
        }
    }
    let sub_index = matched_index.or(first_index)
        .ok_or_else(|| "no subtitle stream found".to_string())?;

    // Extracts the FULL, untrimmed subtitle stream — no -ss/-t here.
    // See shift_ass_timestamps' own comment for why ffmpeg's own
    // "-ss"/"-t" trim can't be trusted to rebase an .ass file's embedded
    // Dialogue timestamps to start at 0 (it doesn't — confirmed via real
    // command logs from a genuine "export succeeds, no subtitles appear"
    // bug). Timestamps are shifted manually afterward instead, once, on
    // the extracted file.
    let extract_args: Vec<String> = vec![
        "-y".into(), "-i".into(), path.to_string(),
        "-map".into(), format!("0:{sub_index}"),
        "-c:s".into(), "ass".into(),
        ass_path.to_string_lossy().to_string(),
    ];

    // Font extraction is independent of the ASS extraction below —
    // neither reads the other's output, both just read from the same
    // source file and write to different destinations — so it's spawned
    // as a separate task to run concurrently rather than waiting for ASS
    // extraction to finish first. Low risk: no shared mutable state
    // between them, and font extraction was already best-effort before
    // this change (a failure here was already swallowed silently),
    // which is preserved — this task's own errors are simply treated as
    // "no fonts found" the same way they already were.
    let font_app = app.clone();
    let font_path = path.to_string();
    let font_temp_dir = temp_dir.to_path_buf();
    let font_task = tauri::async_runtime::spawn(async move {
        extract_fonts(&font_app, &font_path, &font_temp_dir).await
    });

    run_bin(app, "ffmpeg", &extract_args, None).await?;
    if let Some((in_time, duration)) = trim {
        shift_ass_timestamps(&ass_path, in_time, duration)?;
    }

    let font_paths = font_task.await.unwrap_or_default();

    Ok(ExtractedSubtitles { ass_path, font_paths })
}

// Split out of extract_subtitles_to specifically so it can run
// concurrently with ASS extraction there (see the comment at that call
// site) — best-effort throughout, matching the behavior before this
// split: any failure along the way just results in an empty Vec rather
// than propagating an error, since a file with no fonts (or fonts
// already present on the system) still works fine, just with less
// accurate font matching.
async fn extract_fonts(app: &AppHandle, path: &str, temp_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let font_probe = match run_bin(app, "ffprobe", &[
        "-v".into(), "error".into(),
        "-select_streams".into(), "t".into(),
        "-show_entries".into(), "stream=index:stream_tags=filename".into(),
        "-of".into(), "csv=p=0".into(),
        path.to_string(),
    ], None).await {
        Ok(out) => out,
        Err(_) => return vec![],
    };

    let mut dump_args: Vec<String> = vec!["-y".into()];
    let mut pending: Vec<std::path::PathBuf> = vec![];
    for line in String::from_utf8_lossy(&font_probe).lines() {
        let mut parts = line.splitn(2, ',');
        let (Some(idx_str), Some(filename)) = (parts.next(), parts.next()) else { continue };
        let Ok(idx) = idx_str.trim().parse::<u32>() else { continue };
        let filename = filename.trim();
        if filename.is_empty() || filename.contains('/') || filename.contains('\\') || filename.contains("..") {
            continue;
        }
        let out_font = temp_dir.join(filename);
        dump_args.push(format!("-dump_attachment:{idx}"));
        dump_args.push(out_font.to_string_lossy().to_string());
        pending.push(out_font);
    }

    let mut font_paths = vec![];
    if !pending.is_empty() {
        dump_args.push("-i".into());
        dump_args.push(path.to_string());
        dump_args.push("-f".into());
        dump_args.push("null".into());
        dump_args.push("-".into());
        let _ = run_bin(app, "ffmpeg", &dump_args, None).await; // best-effort
        for p in pending {
            if p.exists() {
                font_paths.push(p);
            }
        }
    }
    font_paths
}

// Used for the live EDITING preview specifically: a plain browser
// <video> element cannot render embedded ASS/SSA tracks from an MKV at
// all (not a bug — Chromium's media pipeline just doesn't support it),
// so libass-wasm (subtitles-octopus.js, vendored under src/lib/) renders
// them separately as an overlay, reading these extracted files directly.
#[tauri::command]
async fn extract_subtitles_for_preview(app: AppHandle, path: String, subtitle_lang: Option<String>, subtitle_external_file: Option<String>) -> Result<SubtitlePreviewData, String> {
    let temp_dir = std::env::temp_dir().join(format!("klippit-subs-preview-{}", std::process::id()));
    // Clear any leftovers from a previously previewed file in this same
    // session (switching files via single-instance re-seed) rather than
    // letting old fonts/ass files accumulate here indefinitely.
    let _ = std::fs::remove_dir_all(&temp_dir);
    let extracted = extract_subtitles_to(
        &app, &path, &temp_dir, None,
        subtitle_external_file.as_deref(), subtitle_lang.as_deref(),
    ).await?;
    // Forward slashes, not whatever PathBuf naturally formats as — same
    // fix as subtitle_filter() in the export path and for the same
    // reason: Tauri's convertFileSrc() (called on these paths from the
    // JS side) mishandles Windows backslashes, producing a malformed
    // asset URL with a literal %5C in it instead of a working path —
    // confirmed directly from a real "Loading data file ... failed"
    // error containing exactly that in testing.
    Ok(SubtitlePreviewData {
        ass_path: extracted.ass_path.to_string_lossy().replace('\\', "/"),
        font_paths: extracted.font_paths.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect(),
    })
}

// Opens a small dedicated window playing the given file, with native
// <video controls> — used by the Play button so review happens inside
// Klippit rather than handing off to the OS default media player.
#[tauri::command]
async fn open_review_window(app: AppHandle, path: String) -> Result<(), String> {
    let path_json = serde_json::to_string(&path).map_err(|e| e.to_string())?;

    // Reuse an existing review window rather than closing and
    // recreating one — confirmed as a real race: .close() doesn't wait
    // for the old window to actually finish tearing down before
    // returning, so a fast second export + Play could try creating a
    // new window with the same label while the old one was still
    // technically registered, failing with "a webview with label
    // `review` already exists." Reusing sidesteps that race entirely
    // rather than trying to work around it with a wait/retry loop.
    if let Some(existing) = app.get_webview_window("review") {
        let script = format!("window.applyReviewPath({path_json});");
        existing.eval(&script).map_err(|e| e.to_string())?;
        let _ = existing.center();
        let _ = existing.set_always_on_top(true);
        let _ = existing.set_always_on_top(false);
        let _ = existing.set_focus();
        return Ok(());
    }

    let script = format!("window.__KLIPPIT_REVIEW_PATH__ = {path_json};");

    let window = tauri::WebviewWindowBuilder::new(&app, "review", tauri::WebviewUrl::App("review.html".into()))
        .title("Klippit — Preview")
        .inner_size(640.0, 480.0)
        .initialization_script(&script)
        .build()
        .map_err(|e| e.to_string())?;

    // Same "raise once, don't stay pinned" toggle as the main window —
    // permanent always_on_top(true) here was reported as genuinely bad
    // behavior: this window would keep forcing itself above whatever
    // else the person switched to (a browser, a file explorer, anything)
    // for as long as it stayed open. This still reliably appears above
    // other applications like VLC at the moment it opens (the actual
    // reason this existed), then behaves like any other normal window.
    let _ = window.center();
    let _ = window.set_always_on_top(true);
    let _ = window.set_always_on_top(false);
    let _ = window.set_focus();

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

// ---------- setup: environment variable + companion script installation ----------
// Runs two ways: once, headlessly, right after install (see
// installer-hooks.nsh calling `klippit.exe --setup`), and again in the
// background every time Klippit launches normally (see .setup() in
// main() below) as a self-healing safety net — covers both "the
// installer's assumed resource-folder layout doesn't quite match this
// build" and "the person moved/reinstalled Klippit since the last
// install." Always overwriting the copied scripts on every run is
// deliberate, not wasteful: it also means an updated script shipped in
// a newer Klippit version automatically reaches the person's mpv/VLC
// config on their very next launch, rather than requiring a manual
// re-copy — a real recurring friction point earlier in this project.
struct SetupResult {
    env_var: Result<(), String>,
    mpv_script: Result<(), String>,
    vlc_script: Result<(), String>,
    mpv_keybind_config: Result<(), String>,
}

impl SetupResult {
    fn any_failed(&self) -> bool {
        self.env_var.is_err() || self.mpv_script.is_err() || self.vlc_script.is_err() || self.mpv_keybind_config.is_err()
    }
}

fn fmt_result(label: &str, r: &Result<(), String>) -> String {
    match r {
        Ok(()) => format!("{label}: OK"),
        Err(e) => format!("{label}: FAILED — {e}"),
    }
}

// UNVERIFIED which of these Tauri's NSIS bundler actually uses for
// `bundle.resources` — "resources" alongside the exe is the documented
// convention as I understand it, but trying the exe's own directory too
// costs nothing and covers a plausible alternate layout.
fn resource_dir_candidates(exe_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![exe_dir.join("resources"), exe_dir.to_path_buf()]
}

fn find_resource(exe_dir: &std::path::Path, rel_path: &str) -> Option<std::path::PathBuf> {
    resource_dir_candidates(exe_dir)
        .into_iter()
        .map(|base| base.join(rel_path))
        .find(|candidate| candidate.exists())
}

fn set_klippit_path_env(exe_path: &std::path::Path) -> Result<(), String> {
    let exe_str = exe_path.to_string_lossy().to_string();
    // setx over a registry-writing crate: no new dependency, and this is
    // the exact same mechanism already documented and manually verified
    // working throughout this project — high confidence, unlike most of
    // what surrounds it here.
    let mut cmd = std::process::Command::new("setx");
    cmd.arg("KLIPPIT_PATH").arg(&exe_str);
    // Without this, setx — a console application — pops open a visible
    // console window, since klippit.exe itself is a GUI app with no
    // console of its own to share. Confirmed as a real failure mode:
    // closing that window before setx finished writing killed it
    // mid-operation. No window at all means nothing for anyone to
    // accidentally close.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run setx: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

fn install_script(exe_dir: &std::path::Path, resource_rel: &str, dest_dir: std::path::PathBuf, dest_name: &str) -> Result<(), String> {
    let src = find_resource(exe_dir, resource_rel)
        .ok_or_else(|| format!("bundled resource not found: {resource_rel} (tried resources/ and the exe's own directory)"))?;
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("couldn't create {}: {e}", dest_dir.display()))?;
    let dest = dest_dir.join(dest_name);
    std::fs::copy(&src, &dest).map_err(|e| format!("couldn't copy to {}: {e}", dest.display()))?;
    Ok(())
}

// ---------- user-editable settings (mpv keybind, config folder override) ----------
// Klippit's first-ever persisted settings — previously nothing needed
// remembering between runs. Kept as a plain JSON file rather than the
// registry: simple to read/write with std::fs alone, no new dependency,
// and easy for a person to inspect or delete by hand if something goes
// wrong.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct KlippitSettings {
    #[serde(default = "default_mpv_keybind")]
    mpv_keybind: String,
    // None = use the default %APPDATA%\mpv. Set when someone's mpv uses
    // portable_config (a folder next to mpv.exe itself, which mpv
    // prefers over %APPDATA% when present) — confirmed as a real gap via
    // actual user feedback: the previous hardcoded %APPDATA% path
    // silently wrote the script somewhere portable-mode mpv never looks,
    // with no error or indication anything was wrong.
    #[serde(default)]
    mpv_config_dir_override: Option<String>,
}
fn default_mpv_keybind() -> String { "c".to_string() }
impl Default for KlippitSettings {
    fn default() -> Self {
        KlippitSettings { mpv_keybind: default_mpv_keybind(), mpv_config_dir_override: None }
    }
}

fn settings_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    std::path::PathBuf::from(appdata).join("Klippit").join("settings.json")
}

fn load_settings() -> KlippitSettings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(settings: &KlippitSettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

// Writes mpv's script-opts config file for clip-trigger.lua — the
// mechanism that lets the keybind be changed without ever editing the
// script itself. mpv_config_dir is whatever run_setup() resolved (either
// the default %APPDATA%\mpv or the person's override), matching wherever
// the script itself just got copied to.
fn write_mpv_keybind_config(mpv_config_dir: &std::path::Path, keybind: &str) -> Result<(), String> {
    let dir = mpv_config_dir.join("script-opts");
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = dir.join("clip-trigger.conf");
    std::fs::write(&path, format!("key={keybind}\n")).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

fn run_setup() -> SetupResult {
    let exe_path = std::env::current_exe().unwrap_or_default();
    let exe_dir = exe_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let settings = load_settings();

    let env_var = set_klippit_path_env(&exe_path);

    // mpv_config_dir: the person's override (for portable_config setups,
    // or any other nonstandard mpv config location) if one is set via
    // Klippit's Settings panel, otherwise the default %APPDATA%\mpv.
    let mpv_config_dir = settings.mpv_config_dir_override
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(&appdata).join("mpv"));

    let mpv_dest = mpv_config_dir.join("scripts");
    let mpv_script = install_script(&exe_dir, "mpv-scripts/clip-trigger.lua", mpv_dest, "clip-trigger.lua");
    let mpv_keybind_config = write_mpv_keybind_config(&mpv_config_dir, &settings.mpv_keybind);

    let vlc_dest = std::path::PathBuf::from(&appdata).join("vlc").join("lua").join("extensions");
    let vlc_script = install_script(&exe_dir, "vlc-scripts/klippit-extension.lua", vlc_dest, "klippit.lua");

    SetupResult { env_var, mpv_script, vlc_script, mpv_keybind_config }
}

// Overwritten on every setup run (both the installer's one-shot --setup
// call and the background check on every normal launch) — always shows
// the latest status, not a growing history, since only "is it correct
// right now" matters here.
fn write_setup_log(result: &SetupResult) {
    let lines = vec![
        fmt_result("KLIPPIT_PATH env var", &result.env_var),
        fmt_result("mpv script", &result.mpv_script),
        fmt_result("mpv keybind config", &result.mpv_keybind_config),
        fmt_result("VLC extension", &result.vlc_script),
    ];
    let log_path = std::env::temp_dir().join("klippit-setup.log");
    let _ = std::fs::write(log_path, lines.join("\n"));
}

// Klippit's first Settings panel — lets the mpv keybind and mpv config
// folder (for portable_config setups) be changed from inside the app
// itself, rather than requiring anyone to edit clip-trigger.lua or
// input.conf by hand. Both commands are thin wrappers around the same
// settings/run_setup machinery already used for the background
// self-healing check — saving here just means the next self-heal (and
// this immediate reinstall) picks up the new values.
#[tauri::command]
async fn get_settings() -> KlippitSettings {
    load_settings()
}

#[tauri::command]
async fn save_settings_and_reinstall(mpv_keybind: String, mpv_config_dir_override: Option<String>) -> Result<String, String> {
    let keybind = if mpv_keybind.trim().is_empty() { default_mpv_keybind() } else { mpv_keybind.trim().to_string() };
    let dir_override = mpv_config_dir_override.filter(|s| !s.trim().is_empty());
    let settings = KlippitSettings { mpv_keybind: keybind, mpv_config_dir_override: dir_override };
    save_settings(&settings)?;

    let result = run_setup();
    write_setup_log(&result);

    Ok(vec![
        fmt_result("KLIPPIT_PATH env var", &result.env_var),
        fmt_result("mpv script", &result.mpv_script),
        fmt_result("mpv keybind config", &result.mpv_keybind_config),
        fmt_result("VLC extension", &result.vlc_script),
    ].join("\n"))
}

fn main() {
    // Headless setup mode — invoked once by the installer right after
    // install finishes (see installer-hooks.nsh). No window, no Tauri
    // Builder at all: just do the work, log the outcome, exit. Kept
    // deliberately independent of Tauri's own machinery (current_exe(),
    // std::fs, std::process — nothing Tauri-specific) so this path works
    // identically whether or not the NSIS hook wiring above it turns out
    // to be exactly right.
    if std::env::args().any(|a| a == "--setup") {
        let result = run_setup();
        write_setup_log(&result);
        std::process::exit(0);
    }

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
                    // Same "raise once, don't stay pinned" toggle as the
                    // initial window creation below — this is exactly
                    // the moment it matters most (mpv/VLC's trigger key
                    // pressed again while Klippit is already open behind
                    // the video player), and exactly where persistent
                    // always-on-top used to cause the most friction
                    // afterward.
                    let _ = window.set_always_on_top(true);
                    let _ = window.set_always_on_top(false);
                    let _ = window.set_focus();
                }
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(ExportState::default())
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
            .inner_size(900.0, 720.0)
            .min_inner_size(700.0, 480.0)
            .resizable(true);

            if let Some(json) = &init_json {
                let script = format!("window.__KLIPPIT_INIT__ = {json};");
                builder = builder.initialization_script(&script);
            }

            let window = builder.build()?;

            // Bring the window to the front once at creation, without
            // leaving it permanently pinned above everything else —
            // that's the actual fix here. The previous
            // always_on_top(true) on the builder kept it forced above
            // every other window for as long as it stayed open, which
            // was reported as genuinely bad behavior: switching to a
            // file explorer, a browser, or any other program while
            // Klippit stayed open meant it kept forcing itself back in
            // front, obstructing whatever the person actually wanted to
            // look at. Toggling always-on-top on then immediately off
            // is a standard trick for "raise this once" without that
            // persistent side effect — it still reliably appears above
            // a borderless-windowed-fullscreen video player (the actual
            // reason this existed) but behaves like any other normal
            // window from that point on: can be covered by other
            // windows, alt-tabbed normally, etc.
            let _ = window.set_always_on_top(true);
            let _ = window.set_always_on_top(false);
            let _ = window.set_focus();

            // Self-healing safety net: re-verify the same things the
            // installer's --setup call already tried, every normal
            // launch too — covers both "the installer hook's assumed
            // resource layout wasn't quite right for this build" and
            // "the person moved or reinstalled Klippit since the last
            // install." Spawned after the window already exists so it
            // adds no perceptible startup delay; only surfaces anything
            // in-app if something actually failed, so a working setup
            // stays invisible rather than nagging every launch.
            tauri::async_runtime::spawn(async move {
                let result = run_setup();
                write_setup_log(&result);
                if result.any_failed() {
                    let msg = format!(
                        "{}\n{}\n{}",
                        fmt_result("KLIPPIT_PATH", &result.env_var),
                        fmt_result("mpv script", &result.mpv_script),
                        fmt_result("VLC extension", &result.vlc_script),
                    );
                    let _ = window.eval(&format!(
                        "window.__klippitSetupWarning && window.__klippitSetupWarning({});",
                        serde_json::to_string(&msg).unwrap_or_else(|_| "\"setup check failed\"".into())
                    ));
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_video_metadata,
            export_clip,
            extract_frame,
            extract_subtitles_for_preview,
            open_review_window,
            get_settings,
            save_settings_and_reinstall,
            cancel_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
