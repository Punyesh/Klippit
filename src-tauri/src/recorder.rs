use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tauri_plugin_shell::ShellExt;

#[cfg(windows)]
use crate::native_capture::{CaptureRegion, NativeCaptureSession, NativeSegment};

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;
const CAPTURE_BACKEND: &str = "native-wgc";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderConfig {
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default = "default_quality")]
    pub quality: String,
    #[serde(default = "default_true")]
    pub draw_cursor: bool,
    #[serde(default)]
    pub system_audio: bool,
    #[serde(default)]
    pub microphone: bool,
}
fn default_fps() -> u32 { 60 }
fn default_codec() -> String { "auto".into() }
fn default_quality() -> String { "high".into() }
fn default_true() -> bool { true }
impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            fps: default_fps(), codec: default_codec(), quality: default_quality(),
            draw_cursor: true, system_audio: false, microphone: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    phase: String,
    elapsed_seconds: f64,
    fps: f64,
    frame_count: u64,
    dropped_frames: u64,
    duplicated_frames: u64,
    encoder: String,
    audio_error: Option<String>,
    capture_error: Option<String>,
}

#[derive(Debug, Clone)]
struct SegmentFiles {
    video: PathBuf,
    system_audio: Option<PathBuf>,
    microphone: Option<PathBuf>,
}

struct ActiveSegment {
    files: SegmentFiles,
    #[cfg(windows)]
    native: NativeSegment,
    audio_stop: Arc<AtomicBool>,
    audio_threads: Vec<JoinHandle<Result<(), String>>>,
    started: Instant,
    encoder: String,
}


struct RecorderInner {
    phase: String,
    config: RecorderConfig,
    temp_dir: Option<PathBuf>,
    segments: Vec<SegmentFiles>,
    active: Option<ActiveSegment>,
    accumulated: Duration,
    next_segment: usize,
    unsaved_recording: Option<PathBuf>,
    unsaved_dir: Option<PathBuf>,
    last_audio_error: Option<String>,
    last_capture_error: Option<String>,
    #[cfg(windows)]
    capture_host: Option<NativeCaptureSession>,
    completed_frames: u64,
    completed_dropped_frames: u64,
    completed_duplicated_frames: u64,
    last_encoder: String,
    last_geometry: Option<RecorderGeometry>,
}
impl Default for RecorderInner {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            config: RecorderConfig::default(),
            temp_dir: None,
            segments: vec![],
            active: None,
            accumulated: Duration::ZERO,
            next_segment: 0,
            unsaved_recording: None,
            unsaved_dir: None,
            last_audio_error: None,
            last_capture_error: None,
            #[cfg(windows)]
            capture_host: None,
            completed_frames: 0,
            completed_dropped_frames: 0,
            completed_duplicated_frames: 0,
            last_encoder: String::new(),
            last_geometry: None,
        }
    }
}

pub struct RecorderState {
    inner: Mutex<RecorderInner>,
}
impl Default for RecorderState {
    fn default() -> Self { Self { inner: Mutex::new(RecorderInner::default()) } }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecorderGeometry {
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    monitor_width: u32,
    monitor_height: u32,
    monitor_name: String,
}

fn now_id() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn recorder_root() -> PathBuf {
    std::env::temp_dir().join("Klippit").join("Recordings").join("Working")
}

fn register_recorder_hotkeys(app: &AppHandle) {
    let shortcuts = app.global_shortcut();
    for key in ["F9", "F10", "F11"] {
        if !shortcuts.is_registered(key) {
            // A different application may already own one of these keys. The
            // recorder remains fully usable from its on-screen controls, so a
            // shortcut conflict must not prevent the recorder from opening.
            let _ = shortcuts.register(key);
        }
    }
}

fn unregister_recorder_hotkeys(app: &AppHandle) {
    let shortcuts = app.global_shortcut();
    for key in ["F9", "F10", "F11"] {
        if shortcuts.is_registered(key) {
            let _ = shortcuts.unregister(key);
        }
    }
}

fn set_excluded_from_capture(window: &tauri::WebviewWindow) {
    // Tauri maps this to the native platform capture-protection mechanism
    // (WDA_EXCLUDEFROMCAPTURE on supported Windows versions). Keep the
    // frame/control bar visible to the user without burning either into the
    // Windows Graphics Capture recording.
    let _ = window.set_content_protected(true);
}

fn position_controls(app: &AppHandle) -> Result<(), String> {
    let frame = app.get_webview_window("recorder-frame").ok_or("recorder frame is not open")?;
    let controls = app.get_webview_window("recorder-controls").ok_or("recorder controls are not open")?;
    let pos = frame.outer_position().map_err(|e| e.to_string())?;
    let size = frame.outer_size().map_err(|e| e.to_string())?;
    let csize = controls.outer_size().map_err(|e| e.to_string())?;
    let monitor = frame.current_monitor().map_err(|e| e.to_string())?.ok_or("selection is not on a monitor")?;
    let mp = monitor.position();
    let ms = monitor.size();

    // Center the controls under the selected region so the capture frame and
    // recorder toolbar read as a single attached tool.
    let mut x = pos.x + (size.width as i32 - csize.width as i32) / 2;
    let mut y = pos.y + size.height as i32 + 8;
    let right = mp.x + ms.width as i32;
    let bottom = mp.y + ms.height as i32;
    if x < mp.x { x = mp.x; }
    if x + csize.width as i32 > right { x = right - csize.width as i32; }
    if y + csize.height as i32 > bottom {
        y = pos.y - csize.height as i32 - 8;
    }
    if y < mp.y { y = mp.y; }
    controls.set_position(PhysicalPosition::new(x, y)).map_err(|e| e.to_string())?;
    Ok(())
}

fn selection_geometry(app: &AppHandle) -> Result<(RecorderGeometry, usize, i32, i32), String> {
    let frame = app.get_webview_window("recorder-frame").ok_or("recorder frame is not open")?;
    let pos = frame.inner_position().map_err(|e| e.to_string())?;
    let size = frame.inner_size().map_err(|e| e.to_string())?;
    let monitor = frame.current_monitor().map_err(|e| e.to_string())?.ok_or("selection is not on a monitor")?;
    let mp = monitor.position();
    let ms = monitor.size();

    if pos.x < mp.x || pos.y < mp.y || pos.x + size.width as i32 > mp.x + ms.width as i32 || pos.y + size.height as i32 > mp.y + ms.height as i32 {
        return Err("Keep the recording selection entirely inside one monitor.".into());
    }
    let width = size.width & !1;
    let height = size.height & !1;
    if width < 160 || height < 90 { return Err("Recording region is too small (minimum 160×90).".into()); }

    let monitors = frame.available_monitors().map_err(|e| e.to_string())?;
    let fallback_idx = monitors.iter().position(|m| m.position() == monitor.position() && m.size() == monitor.size()).unwrap_or(0);
    let monitor_name = monitor.name().cloned().unwrap_or_else(|| format!("Monitor {}", fallback_idx + 1));
    // Keep a stable zero-based monitor index for diagnostics/backward-compatible
    // geometry callers. Native capture itself matches the Windows display name.
    // fall back to Tauri's monitor order if a driver supplies another name.
    let upper_name = monitor_name.to_ascii_uppercase();
    let monitor_idx = if upper_name.contains("DISPLAY") {
        upper_name.rsplit("DISPLAY").next()
            .and_then(|n| n.trim_matches(|c: char| !c.is_ascii_digit()).parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
            .unwrap_or(fallback_idx)
    } else {
        fallback_idx
    };
    let offset_x = pos.x - mp.x;
    let offset_y = pos.y - mp.y;
    Ok((RecorderGeometry {
        width, height, x: pos.x, y: pos.y,
        monitor_width: ms.width, monitor_height: ms.height,
        monitor_name,
    }, monitor_idx, offset_x, offset_y))
}

#[cfg(windows)]
fn ensure_native_capture(inner: &mut RecorderInner, monitor_name: &str, draw_cursor: bool) -> Result<(), String> {
    let matches = inner.capture_host.as_ref()
        .map(|host| host.matches(monitor_name, draw_cursor) && !host.is_finished())
        .unwrap_or(false);
    if matches { return Ok(()); }

    if let Some(old) = inner.capture_host.take() {
        if let Err(e) = old.stop() {
            log_recorder_message(&format!("native-capture-stop-warning {e}"));
        }
    }
    let host = NativeCaptureSession::start(monitor_name, draw_cursor)?;
    log_recorder_message(&format!("native-capture-prepared monitor={} cursor={}", monitor_name, draw_cursor));
    inner.capture_host = Some(host);
    Ok(())
}

#[cfg(not(windows))]
fn ensure_native_capture(_inner: &mut RecorderInner, _monitor_name: &str, _draw_cursor: bool) -> Result<(), String> {
    Err("Klippit's native screen recorder is Windows-only.".into())
}

fn stop_native_capture(inner: &mut RecorderInner) {
    #[cfg(windows)]
    if let Some(host) = inner.capture_host.take() {
        if let Err(e) = host.stop() {
            log_recorder_message(&format!("native-capture-stop-warning {e}"));
        }
    }
}

#[tauri::command]
pub async fn open_recorder(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<(), String> {
    log_recorder_message("capture-backend native Windows Graphics Capture (prepared session)");
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.unsaved_recording.is_some() {
            return Err("The current screen recording is still unsaved. Save/export it or discard the original before starting another recording.".into());
        }
        if let Some(old_dir) = inner.unsaved_dir.take() {
            let _ = fs::remove_dir_all(old_dir);
        }
    }
    if let Some(frame) = app.get_webview_window("recorder-frame") {
        register_recorder_hotkeys(&app);
        let _ = frame.show(); let _ = frame.set_focus();
        if let Some(c) = app.get_webview_window("recorder-controls") { let _ = c.show(); }
        let _ = position_controls(&app);
        let (geom, _, _, _) = selection_geometry(&app)?;
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let cursor = inner.config.draw_cursor;
        ensure_native_capture(&mut inner, &geom.monitor_name, cursor)?;
        return Ok(());
    }

    let primary = app.primary_monitor().map_err(|e| e.to_string())?.ok_or("no monitor found")?;
    let mp = primary.position();
    let ms = primary.size();
    let remembered = state.inner.lock().map_err(|e| e.to_string())?.last_geometry.clone();
    let (w, h, x, y) = if let Some(g) = remembered {
        (g.width.max(160), g.height.max(90), g.x, g.y)
    } else {
        let w = 960u32.min(ms.width.saturating_sub(40)).max(320);
        let h = 540u32.min(ms.height.saturating_sub(180)).max(180);
        let x = mp.x + (ms.width as i32 - w as i32) / 2;
        let y = mp.y + (ms.height as i32 - h as i32) / 2 - 40;
        (w, h, x, y)
    };

    let frame = WebviewWindowBuilder::new(&app, "recorder-frame", WebviewUrl::App("recorder-frame.html".into()))
        .title("Klippit — Recording region")
        .inner_size(w as f64, h as f64)
        .min_inner_size(160.0, 90.0)
        .position(x as f64, y as f64)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .closable(false)
        .resizable(true)
        .build().map_err(|e| e.to_string())?;
    set_excluded_from_capture(&frame);

    let controls = WebviewWindowBuilder::new(&app, "recorder-controls", WebviewUrl::App("recorder.html".into()))
        .title("Klippit — Recorder")
        .inner_size(590.0, 190.0)
        .min_inner_size(560.0, 180.0)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .closable(false)
        .resizable(false)
        .build().map_err(|e| e.to_string())?;
    set_excluded_from_capture(&controls);
    register_recorder_hotkeys(&app);
    let _ = position_controls(&app);
    let _ = controls.set_focus();

    if let Some(main) = app.get_webview_window("main") { let _ = main.hide(); }

    // Prepare capture now, while the user is positioning the frame. This is
    // the key latency fix: Start only creates an encoder and begins consuming
    // an already-live frame stream.
    let (geom, _, _, _) = selection_geometry(&app)?;
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let cursor = inner.config.draw_cursor;
        if let Err(e) = ensure_native_capture(&mut inner, &geom.monitor_name, cursor) {
            inner.last_capture_error = Some(e.clone());
            close_recorder_windows(&app);
            if let Some(main) = app.get_webview_window("main") { let _ = main.show(); }
            return Err(e);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn recorder_sync_controls(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<RecorderGeometry, String> {
    let _ = position_controls(&app);
    let (geometry, _, _, _) = selection_geometry(&app)?;
    let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
    if inner.phase == "idle" || inner.phase == "ready" {
        let cursor = inner.config.draw_cursor;
        if let Err(err) = ensure_native_capture(&mut inner, &geometry.monitor_name, cursor) {
            inner.last_capture_error = Some(err.clone());
            return Err(err);
        }
    }
    Ok(geometry)
}

#[tauri::command]
pub fn recorder_set_config(app: AppHandle, state: tauri::State<'_, RecorderState>, config: RecorderConfig) -> Result<(), String> {
    validate_config(&config)?;
    let restart_capture = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.phase != "idle" && inner.phase != "ready" { return Err("Recording options are locked after recording starts.".into()); }
        let restart = inner.config.draw_cursor != config.draw_cursor;
        inner.config = config;
        restart
    };
    if restart_capture {
        let (geometry, _, _, _) = selection_geometry(&app)?;
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let cursor = inner.config.draw_cursor;
        ensure_native_capture(&mut inner, &geometry.monitor_name, cursor)?;
    }
    Ok(())
}

fn validate_config(config: &RecorderConfig) -> Result<(), String> {
    if ![24,30,60,120].contains(&config.fps) { return Err("FPS must be 24, 30, 60, or 120.".into()); }
    if !["auto","h264","hevc"].contains(&config.codec.as_str()) { return Err("Unsupported recorder codec.".into()); }
    if !["high","balanced","small"].contains(&config.quality.as_str()) { return Err("Unsupported recorder quality preset.".into()); }
    Ok(())
}

fn quality_bitrate(config: &RecorderConfig, width: u32, height: u32) -> u64 {
    let pixels = width as f64 * height as f64;
    let fps_factor = config.fps as f64 / 60.0;
    let base_1080_60 = match config.quality.as_str() {
        "high" => 20_000_000.0,
        "small" => 6_000_000.0,
        _ => 12_000_000.0,
    };
    (base_1080_60 * (pixels / (1920.0 * 1080.0)).max(0.18) * fps_factor.max(0.5)).round() as u64
}

fn recorder_log_path_buf() -> PathBuf {
    std::env::temp_dir().join("klippit-recorder.log")
}

fn log_recorder_message(message: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(recorder_log_path_buf()) {
        for line in message.lines() {
            let _ = writeln!(f, "recorder {line}");
        }
    }
}

fn log_recorder_command(args: &[String]) {
    // FFmpeg remains downstream for audio muxing/segment concatenation/export,
    // but never owns the live Windows capture session anymore.
    log_recorder_message(&format!("ffmpeg-finalize {}", args.join(" ")));
}

fn redact_diagnostic_paths(text: &str) -> String {
    let mut out = text.to_string();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            out = out.replace(&profile, "%USERPROFILE%");
            out = out.replace(&profile.replace('\\', "/"), "%USERPROFILE%");
        }
    }
    let temp = std::env::temp_dir().to_string_lossy().to_string();
    if !temp.is_empty() {
        out = out.replace(&temp, "%TEMP%");
        out = out.replace(&temp.replace('\\', "/"), "%TEMP%");
    }
    out
}

fn recent_recorder_log() -> String {
    let Ok(text) = fs::read_to_string(recorder_log_path_buf()) else { return String::new(); };
    let lines: Vec<&str> = text.lines().filter(|line| line.starts_with("recorder ")).collect();
    if lines.is_empty() { return String::new(); }
    let start = lines.iter().rposition(|line| line.starts_with("recorder session-start "))
        .unwrap_or_else(|| lines.len().saturating_sub(50));
    let mut scoped = lines[start..].to_vec();
    if scoped.len() > 60 { scoped.drain(..scoped.len()-60); }
    scoped.join("\n")
}

#[cfg(windows)]
fn spawn_audio_capture(path: PathBuf, loopback: bool, stop: Arc<AtomicBool>) -> JoinHandle<Result<(), String>> {
    std::thread::spawn(move || {
        use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};
        wasapi::initialize_mta().ok().map_err(|e| e.to_string())?;
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let device_direction = if loopback { Direction::Render } else { Direction::Capture };
        let device = enumerator.get_default_device(&device_direction).map_err(|e| e.to_string())?;
        let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
        let fmt = WaveFormat::new(32, 32, &SampleType::Float, AUDIO_RATE as usize, AUDIO_CHANNELS as usize, None);
        let mode = StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 200_000 };
        client.initialize_client(&fmt, &Direction::Capture, &mode).map_err(|e| e.to_string())?;
        let capture = client.get_audiocaptureclient().map_err(|e| e.to_string())?;
        let mut queue = VecDeque::<u8>::new();
        let mut file = File::create(&path).map_err(|e| e.to_string())?;
        client.start_stream().map_err(|e| e.to_string())?;
        while !stop.load(Ordering::SeqCst) {
            capture.read_from_device_to_deque(&mut queue).map_err(|e| e.to_string())?;
            if queue.len() >= 4096 {
                // Avoid allocating/copying a fresh Vec every ~8 ms. WASAPI
                // already gives us bytes in a VecDeque; write its two backing
                // slices directly, then clear the queue.
                let (a, b) = queue.as_slices();
                file.write_all(a).map_err(|e| e.to_string())?;
                file.write_all(b).map_err(|e| e.to_string())?;
                queue.clear();
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        let _ = capture.read_from_device_to_deque(&mut queue);
        if !queue.is_empty() {
            let (a, b) = queue.as_slices();
            file.write_all(a).map_err(|e| e.to_string())?;
            file.write_all(b).map_err(|e| e.to_string())?;
        }
        let _ = client.stop_stream();
        file.flush().map_err(|e| e.to_string())?;
        wasapi::deinitialize();
        Ok(())
    })
}

#[cfg(not(windows))]
fn spawn_audio_capture(_path: PathBuf, _loopback: bool, _stop: Arc<AtomicBool>) -> JoinHandle<Result<(), String>> {
    std::thread::spawn(|| Err("WASAPI recording is Windows-only".into()))
}

fn begin_segment(app: &AppHandle, inner: &mut RecorderInner) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (app, inner);
        return Err("Klippit's native screen recorder is Windows-only.".into());
    }

    #[cfg(windows)]
    {
        let (geom, _monitor_idx, off_x, off_y) = selection_geometry(app)?;
        if off_x < 0 || off_y < 0 {
            return Err("Keep the recording selection entirely inside one monitor.".into());
        }
        let region = CaptureRegion {
            x: off_x as u32,
            y: off_y as u32,
            width: geom.width,
            height: geom.height,
        };
        let dir = inner.temp_dir.clone().ok_or("recorder working directory is missing")?;
        let idx = inner.next_segment;
        let video = dir.join(format!("segment-{idx:03}.mp4"));
        let _ = fs::remove_file(&video);

        // Capture is normally already running because open_recorder() prepares
        // it while the user positions the region. This only restarts the host
        // when the region moved to another monitor or the cursor setting changed.
        let draw_cursor = inner.config.draw_cursor;
        ensure_native_capture(inner, &geom.monitor_name, draw_cursor)?;
        let bitrate = quality_bitrate(&inner.config, geom.width, geom.height)
            .min(u32::MAX as u64) as u32;
        let native = inner.capture_host.as_ref()
            .ok_or("native capture session is unavailable")?
            .start_segment(&video, region, inner.config.fps, &inner.config.codec, bitrate)?;
        let encoder = native.encoder_label().to_string();
        let started = native.started();

        let audio_stop = Arc::new(AtomicBool::new(false));
        let system_audio = inner.config.system_audio.then(|| dir.join(format!("segment-{idx:03}-system.f32")));
        let microphone = inner.config.microphone.then(|| dir.join(format!("segment-{idx:03}-mic.f32")));
        let mut audio_threads = vec![];
        if let Some(path) = system_audio.clone() {
            let _ = fs::remove_file(&path);
            audio_threads.push(spawn_audio_capture(path, true, audio_stop.clone()));
        }
        if let Some(path) = microphone.clone() {
            let _ = fs::remove_file(&path);
            audio_threads.push(spawn_audio_capture(path, false, audio_stop.clone()));
        }

        inner.next_segment += 1;
        inner.last_encoder = encoder.clone();
        inner.active = Some(ActiveSegment {
            files: SegmentFiles { video, system_audio, microphone },
            native,
            audio_stop,
            audio_threads,
            started,
            encoder: encoder.clone(),
        });
        log_recorder_message(&format!(
            "native-segment-start index={} region={}x{}@{},{} fps={} encoder={}",
            idx, geom.width, geom.height, off_x, off_y, inner.config.fps, encoder
        ));
        Ok(())
    }
}

#[tauri::command]
pub async fn recorder_start(app: AppHandle, state: tauri::State<'_, RecorderState>, config: RecorderConfig) -> Result<(), String> {
    validate_config(&config)?;
    let root = recorder_root();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let dir = root.join(format!("rec-{}-{}", std::process::id(), now_id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    log_recorder_message(&format!(
        "session-start id={} backend={} fps={} codec={} quality={}",
        dir.file_name().and_then(|s| s.to_str()).unwrap_or("unknown"),
        CAPTURE_BACKEND, config.fps, config.codec, config.quality
    ));

    let start_result = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.phase != "idle" && inner.phase != "ready" {
            return Err("Recorder is already active.".into());
        }
        inner.phase = "starting".into();
        inner.config = config;
        inner.temp_dir = Some(dir.clone());
        inner.segments.clear();
        inner.accumulated = Duration::ZERO;
        inner.next_segment = 0;
        inner.last_audio_error = None;
        inner.last_capture_error = None;
        inner.completed_frames = 0;
        inner.completed_dropped_frames = 0;
        inner.completed_duplicated_frames = 0;
        inner.last_encoder.clear();
        begin_segment(&app, &mut inner)
    };

    if let Err(err) = start_result {
        log_recorder_message(&format!("start-error {err}"));
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.phase = "idle".into();
        inner.last_capture_error = Some(err.clone());
        inner.active = None;
        inner.temp_dir = None;
        let _ = fs::remove_dir_all(&dir);
        return Err(err);
    }

    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.phase = "recording".into();
    }
    if let Some(frame) = app.get_webview_window("recorder-frame") {
        let _ = frame.set_ignore_cursor_events(true);
    }
    Ok(())
}

async fn finish_active_segment(state: &RecorderState) -> Result<(), String> {
    let mut active = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.active.take().ok_or("No active recording segment.")?
    };
    let elapsed = active.started.elapsed();
    active.audio_stop.store(true, Ordering::SeqCst);

    #[cfg(windows)]
    let pre_stats = active.native.stats();
    #[cfg(windows)]
    let native_result = active.native.finish();

    let mut audio_err = None;
    for handle in active.audio_threads.drain(..) {
        match handle.join() {
            Ok(Ok(())) => {},
            Ok(Err(e)) => audio_err = Some(e),
            Err(_) => audio_err = Some("audio capture thread crashed".into()),
        }
    }

    #[cfg(windows)]
    let (frames, drops, dups, capture_updates, native_error) = match native_result {
        Ok(stats) => (stats.frame_count, stats.dropped_frames, stats.duplicated_frames, stats.capture_updates, None),
        Err(err) => (pre_stats.frame_count, pre_stats.dropped_frames, pre_stats.duplicated_frames, pre_stats.capture_updates, Some(err)),
    };
    #[cfg(not(windows))]
    let (frames, drops, dups, capture_updates, native_error) =
        (0u64, 0u64, 0u64, 0u64, Some("native video recording is Windows-only".to_string()));

    let video_ok = fs::metadata(&active.files.video).map(|m| m.len() > 1024).unwrap_or(false);
    if native_error.is_some() || !video_ok {
        let detail = native_error.unwrap_or_else(|| "native video encoder produced no usable segment".into());
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.phase = "failed".into();
        inner.last_capture_error = Some(detail.clone());
        if let Some(err) = audio_err { inner.last_audio_error = Some(err); }
        log_recorder_message(&format!("native-segment-error {detail}"));
        return Err(format!("screen capture stopped unexpectedly: {detail}"));
    }

    let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
    inner.accumulated += elapsed;
    inner.completed_frames += frames;
    inner.completed_dropped_frames += drops;
    inner.completed_duplicated_frames += dups;
    inner.last_encoder = active.encoder;
    inner.segments.push(active.files);
    if let Some(err) = audio_err { inner.last_audio_error = Some(err); }
    log_recorder_message(&format!(
        "native-segment-finished frames={} dropped={} duplicated={} captureUpdates={}",
        frames, drops, dups, capture_updates
    ));
    Ok(())
}

#[tauri::command]
pub async fn recorder_pause(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<(), String> {
    {
        let inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.phase != "recording" { return Err("Recorder is not recording.".into()); }
    }
    finish_active_segment(&state).await?;
    let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
    inner.phase = "paused".into();
    let _ = app.emit_to("recorder-controls", "recorder-state-changed", "paused");
    Ok(())
}

#[tauri::command]
pub async fn recorder_resume(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<(), String> {
    let result = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.phase != "paused" { return Err("Recorder is not paused.".into()); }
        inner.phase = "starting".into();
        begin_segment(&app, &mut inner)
    };
    match result {
        Ok(()) => {
            let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
            inner.phase = "recording".into();
            Ok(())
        }
        Err(err) => {
            let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
            inner.phase = "paused".into();
            inner.last_capture_error = Some(err.clone());
            Err(err)
        }
    }
}

async fn mux_segment_audio(app:&AppHandle, files:&SegmentFiles, idx:usize, dir:&Path) -> Result<PathBuf,String> {
    // Native live capture writes a finalized MP4 segment. Always remux to MKV
    // before concat so every segment has the same container regardless of
    // whether audio capture was enabled.
    let out=dir.join(format!("segment-{idx:03}-mux.mkv"));
    let mut a=vec!["-y".into(),"-i".into(),files.video.to_string_lossy().to_string()];

    if files.system_audio.is_none() && files.microphone.is_none() {
        a.extend(["-map".into(),"0:v:0".into(),"-c:v".into(),"copy".into(),"-an".into()]);
    } else {
        let mut input_count=0;
        // Keep every paused/resumed segment structurally identical. If WASAPI
        // failed for a requested source in one segment, substitute silence
        // instead of producing an audio-less segment that cannot be stream-copy
        // concatenated with its neighbours. The UI still surfaces audio_error.
        for p in [&files.system_audio,&files.microphone] {
            if let Some(p)=p {
                let has_samples=fs::metadata(p).map(|m|m.len()>0).unwrap_or(false);
                if has_samples {
                    a.extend(["-f".into(),"f32le".into(),"-ar".into(),AUDIO_RATE.to_string(),"-ac".into(),AUDIO_CHANNELS.to_string(),"-i".into(),p.to_string_lossy().to_string()]);
                } else {
                    a.extend(["-f".into(),"lavfi".into(),"-i".into(),format!("anullsrc=r={AUDIO_RATE}:cl=stereo")]);
                }
                input_count+=1;
            }
        }
        if input_count==1 {
            a.extend(["-map".into(),"0:v:0".into(),"-map".into(),"1:a:0".into(),"-c:v".into(),"copy".into(),"-af".into(),"apad".into(),"-c:a".into(),"aac".into(),"-b:a".into(),"192k".into(),"-shortest".into()]);
        } else {
            a.extend(["-filter_complex".into(),"[1:a][2:a]amix=inputs=2:duration=longest:normalize=0,apad[a]".into(),"-map".into(),"0:v:0".into(),"-map".into(),"[a]".into(),"-c:v".into(),"copy".into(),"-c:a".into(),"aac".into(),"-b:a".into(),"192k".into(),"-shortest".into()]);
        }
    }

    a.push(out.to_string_lossy().to_string());
    log_recorder_command(&a);
    let o=app.shell().sidecar("ffmpeg").map_err(|e|e.to_string())?.args(&a).output().await.map_err(|e|e.to_string())?;
    if !o.status.success(){ return Err(format!("failed to finalize recording segment: {}",String::from_utf8_lossy(&o.stderr))); }
    Ok(out)
}

async fn finalize_recording(app:&AppHandle,state:&RecorderState)->Result<PathBuf,String>{
    let (segments,dir)= { let i=state.inner.lock().map_err(|e|e.to_string())?; (i.segments.clone(),i.temp_dir.clone().ok_or("missing recorder temp directory")?) };
    if segments.is_empty(){return Err("Recording contains no completed segments.".into());}
    let mut muxed=vec![];
    for (idx,s) in segments.iter().enumerate(){ muxed.push(mux_segment_audio(app,s,idx,&dir).await?); }
    let final_path=dir.join(format!("Klippit-Recording-{}.mkv",now_id()));
    if muxed.len()==1 { fs::copy(&muxed[0],&final_path).map_err(|e|e.to_string())?; }
    else {
        let list=dir.join("segments.txt"); let mut text=String::new();
        for p in &muxed { text.push_str(&format!("file '{}'\n",p.to_string_lossy().replace('\\',"/").replace('\'',"'\\''"))); }
        fs::write(&list,text).map_err(|e|e.to_string())?;
        let a=vec!["-y".into(),"-f".into(),"concat".into(),"-safe".into(),"0".into(),"-i".into(),list.to_string_lossy().to_string(),"-c".into(),"copy".into(),final_path.to_string_lossy().to_string()];
        log_recorder_command(&a);
        let o=app.shell().sidecar("ffmpeg").map_err(|e|e.to_string())?.args(&a).output().await.map_err(|e|e.to_string())?;
        if !o.status.success(){return Err(format!("failed to join recording segments: {}",String::from_utf8_lossy(&o.stderr)));}
    }
    // Keep only the finalized source. It remains in Working until the user
    // explicitly saves/copies it or discards the original.
    for s in &segments { let _=fs::remove_file(&s.video); if let Some(p)=&s.system_audio{let _=fs::remove_file(p);} if let Some(p)=&s.microphone{let _=fs::remove_file(p);} }
    for p in muxed { if p!=final_path { let _=fs::remove_file(p); } }
    let _=fs::remove_file(dir.join("segments.txt"));
    Ok(final_path)
}

fn close_recorder_windows(app:&AppHandle){
    unregister_recorder_hotkeys(app);
    // These auxiliary windows are intentionally created as non-closable
    // frameless windows. Use destroy() for teardown so a hidden/stale
    // recorder window cannot keep the Tauri process alive after the main
    // Klippit window is closed. close() emits CloseRequested and is the
    // wrong lifecycle primitive for windows the backend owns completely.
    for label in ["recorder-frame", "recorder-controls"] {
        if let Some(w) = app.get_webview_window(label) {
            if let Err(err) = w.destroy() {
                log_recorder_message(&format!("window-destroy-error {label}: {err}"));
            }
        }
    }
}

#[tauri::command]
pub async fn recorder_stop(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<String,String>{
    let (phase, has_active) = {
        let i = state.inner.lock().map_err(|e| e.to_string())?;
        (i.phase.clone(), i.active.is_some())
    };
    if !matches!(phase.as_str(), "recording" | "paused" | "failed") {
        return Err("Recorder is not active.".into());
    }

    // A failed current segment must not make earlier paused segments unusable.
    if has_active {
        if let Err(err) = finish_active_segment(&state).await {
            let completed = state.inner.lock().map_err(|e| e.to_string())?.segments.len();
            if completed == 0 { return Err(err); }
        }
    }
    let completed = state.inner.lock().map_err(|e| e.to_string())?.segments.len();
    if completed == 0 { return Err("Recording contains no completed video to recover.".into()); }

    { state.inner.lock().map_err(|e|e.to_string())?.phase="finalizing".into(); }
    let final_path = match finalize_recording(&app,&state).await {
        Ok(path) => path,
        Err(err) => {
            if let Ok(mut inner) = state.inner.lock() { inner.phase = "paused".into(); }
            return Err(err);
        }
    };
    let remembered_geometry = selection_geometry(&app).ok().map(|x| x.0);
    {
        let mut i=state.inner.lock().map_err(|e|e.to_string())?;
        stop_native_capture(&mut i);
        if remembered_geometry.is_some() { i.last_geometry = remembered_geometry; }
        i.unsaved_recording=Some(final_path.clone());
        i.unsaved_dir=i.temp_dir.take();
        i.segments.clear();
        i.phase="idle".into();
        i.accumulated=Duration::ZERO;
    }
    close_recorder_windows(&app);
    if let Some(main)=app.get_webview_window("main"){
        let path=final_path.to_string_lossy().replace('\\',"/");
        let file=final_path.file_name().and_then(|s|s.to_str()).unwrap_or("Klippit Recording");
        let init=serde_json::json!({"filePath":path,"fileName":file,"startTime":0,"subtitle":{"available":false},"temporaryRecording":true});
        let _=main.eval(&format!("window.applyInit && window.applyInit({});",init));
        let _=main.show(); let _=main.set_focus();
    }
    Ok(final_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn recorder_discard(app:AppHandle,state:tauri::State<'_,RecorderState>)->Result<(),String>{
    let (phase, has_active) = {
        let i=state.inner.lock().map_err(|e|e.to_string())?;
        (i.phase.clone(), i.active.is_some())
    };
    if has_active && matches!(phase.as_str(), "recording" | "failed") {
        // Finalize only so the native encoder/audio threads shut down cleanly;
        // the resulting segment is deleted with the discarded session below.
        let _=finish_active_segment(&state).await;
    }
    let dir={
        let mut i=state.inner.lock().map_err(|e|e.to_string())?;
        i.phase="idle".into();
        i.active=None;
        i.segments.clear();
        i.accumulated=Duration::ZERO;
        i.next_segment=0;
        i.completed_frames=0;
        i.completed_dropped_frames=0;
        i.completed_duplicated_frames=0;
        i.last_audio_error=None;
        i.last_capture_error=None;
        i.last_encoder.clear();
        i.temp_dir.take()
    };
    if let Some(d)=dir{let _=fs::remove_dir_all(d);}
    if let Some(frame)=app.get_webview_window("recorder-frame") {
        let _=frame.set_ignore_cursor_events(false);
    }
    // Keep the prepared native capture session and both recorder windows alive.
    // Discard is a reset-to-ready action; the title-bar X is the explicit exit.
    let (geom, _, _, _) = selection_geometry(&app)?;
    let mut i=state.inner.lock().map_err(|e|e.to_string())?;
    i.last_geometry=Some(geom.clone());
    let cursor=i.config.draw_cursor;
    ensure_native_capture(&mut i,&geom.monitor_name,cursor)?;
    let _=app.emit_to("recorder-controls","recorder-state-changed","idle");
    Ok(())
}

#[tauri::command]
pub async fn recorder_close(app:AppHandle,state:tauri::State<'_,RecorderState>)->Result<(),String>{
    let (phase, has_active) = {
        let i=state.inner.lock().map_err(|e|e.to_string())?;
        (i.phase.clone(), i.active.is_some())
    };
    if has_active && matches!(phase.as_str(), "recording" | "failed") {
        let _=finish_active_segment(&state).await;
    }
    let remembered_geometry=selection_geometry(&app).ok().map(|x|x.0);
    let dir={
        let mut i=state.inner.lock().map_err(|e|e.to_string())?;
        stop_native_capture(&mut i);
        i.phase="idle".into();
        i.active=None;
        i.segments.clear();
        i.accumulated=Duration::ZERO;
        i.next_segment=0;
        i.completed_frames=0;
        i.completed_dropped_frames=0;
        i.completed_duplicated_frames=0;
        i.last_audio_error=None;
        i.last_capture_error=None;
        i.last_encoder.clear();
        if remembered_geometry.is_some(){i.last_geometry=remembered_geometry;}
        i.temp_dir.take()
    };
    if let Some(d)=dir{let _=fs::remove_dir_all(d);}
    close_recorder_windows(&app);
    if let Some(main)=app.get_webview_window("main"){let _=main.show();let _=main.set_focus();}
    Ok(())
}

/// True only when the editor owns a finalized recording that still needs an
/// explicit Save/Discard decision. Used by the native main-window close path.
pub fn has_unsaved_recording(state: &RecorderState) -> bool {
    state.inner.lock().map(|i| i.unsaved_recording.is_some()).unwrap_or(false)
}

/// Best-effort teardown for application exit. Finalized unsaved recordings
/// remain on disk for recovery; an in-progress native segment is finalized if
/// possible before the capture host is stopped.
pub fn shutdown_for_app_exit(app: &AppHandle, state: &RecorderState) {
    let active = if let Ok(mut inner) = state.inner.lock() {
        inner.phase = "idle".into();
        inner.active.take()
    } else { None };

    if let Some(mut active) = active {
        active.audio_stop.store(true, Ordering::SeqCst);
        #[cfg(windows)]
        { let _ = active.native.finish(); }
        for handle in active.audio_threads.drain(..) { let _ = handle.join(); }
    }
    let settled_dir = if let Ok(mut inner) = state.inner.lock() {
        stop_native_capture(&mut inner);
        if inner.unsaved_recording.is_none() { inner.unsaved_dir.take() } else { None }
    } else { None };
    if let Some(dir) = settled_dir { let _ = fs::remove_dir_all(dir); }
    close_recorder_windows(app);
}

#[cfg(windows)]
fn active_native_snapshot(active: &ActiveSegment) -> (f64,u64,u64,u64,u64,bool,Option<String>) {
    let stats=active.native.stats();
    (stats.fps,stats.frame_count,stats.dropped_frames,stats.duplicated_frames,stats.capture_updates,active.native.is_finished(),stats.error)
}
#[cfg(not(windows))]
fn active_native_snapshot(_active: &ActiveSegment) -> (f64,u64,u64,u64,u64,bool,Option<String>) {
    (0.0,0,0,0,0,true,Some("native screen recording is Windows-only".into()))
}

#[tauri::command]
pub fn recorder_status(state:tauri::State<'_,RecorderState>)->Result<RecorderStatus,String>{
    let i=state.inner.lock().map_err(|e|e.to_string())?;
    let mut elapsed=i.accumulated;
    let mut phase=i.phase.clone();
    let mut capture_error=i.last_capture_error.clone();
    let (fps,frames,drops,dups,encoder)=if let Some(a)=&i.active{
        elapsed+=a.started.elapsed();
        let (fps,frames,drops,dups,_updates,finished,error)=active_native_snapshot(a);
        if phase=="recording" && finished {
            phase="failed".into();
            if capture_error.is_none() {
                capture_error=error.or_else(||Some("native video encoder stopped unexpectedly".into()));
            }
        }
        (fps,i.completed_frames+frames,i.completed_dropped_frames+drops,i.completed_duplicated_frames+dups,a.encoder.clone())
    }else{(0.0,i.completed_frames,i.completed_dropped_frames,i.completed_duplicated_frames,i.last_encoder.clone())};
    Ok(RecorderStatus{phase,elapsed_seconds:elapsed.as_secs_f64(),fps,frame_count:frames,dropped_frames:drops,duplicated_frames:dups,encoder,audio_error:i.last_audio_error.clone(),capture_error})
}

#[tauri::command]
pub fn recorder_log_path() -> Result<String, String> {
    let path = recorder_log_path_buf();
    fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn recorder_diagnostics(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<String, String> {
    let i = state.inner.lock().map_err(|e| e.to_string())?;
    let phase=i.phase.clone();
    let config=i.config.clone();
    let temp_dir=i.temp_dir.clone();
    let segments=i.segments.len();
    let encoder=i.last_encoder.clone();
    let audio_error=i.last_audio_error.clone();
    let capture_error=i.last_capture_error.clone();
    let active_stats=i.active.as_ref().map(|active| {
        let (fps,frames,drops,dups,updates,finished,error)=active_native_snapshot(active);
        (active.encoder.clone(),fps,frames,drops,dups,updates,finished,error)
    });
    #[cfg(windows)]
    let host_state=i.capture_host.as_ref().map(|host|(host.is_ready(),host.is_finished(),host.last_error()));
    #[cfg(not(windows))]
    let host_state:Option<(bool,bool,Option<String>)>=None;
    drop(i);

    let geometry = selection_geometry(&app).ok().map(|x| x.0);
    let mut out = String::new();
    out.push_str("Klippit recorder diagnostics\n");
    out.push_str(&format!("Klippit version: {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("Platform: {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
    out.push_str(&format!("Capture backend: {} (native Windows Graphics Capture)\n", CAPTURE_BACKEND));
    out.push_str("Architecture: capture session prewarmed when recorder opens; native fixed-rate encoder duplicates the latest frame\n");
    out.push_str("Live FFmpeg capture: disabled (FFmpeg is used only for final remux/concat)\n");
    out.push_str(&format!("Phase: {phase}\n"));
    out.push_str(&format!("Config: fps={} codec={} quality={} cursor={} systemAudio={} microphone={}\n",
        config.fps, config.codec, config.quality, config.draw_cursor, config.system_audio, config.microphone));
    if let Some(g) = geometry {
        out.push_str(&format!("Region: {}x{} at {},{} on {} ({}x{})\n", g.width, g.height, g.x, g.y, g.monitor_name, g.monitor_width, g.monitor_height));
    }
    if let Some((ready,finished,error))=host_state {
        out.push_str(&format!("Capture session: ready={} finished={}\n",ready,finished));
        if let Some(error)=error { out.push_str(&format!("Capture session error: {error}\n")); }
    }
    out.push_str(&format!("Completed segments: {segments}\n"));
    if !encoder.is_empty() { out.push_str(&format!("Last encoder: {encoder}\n")); }
    if let Some((enc,fps,frames,drops,dups,updates,finished,error))=active_stats {
        out.push_str(&format!("Active encoder: {enc}\nActive stats: fps={fps:.2} frames={frames} dropped={drops} duplicated={dups} captureUpdates={updates} finished={finished}\n"));
        if let Some(error)=error { out.push_str(&format!("Active encoder error: {error}\n")); }
    }
    if let Some(err) = audio_error { out.push_str(&format!("Audio error: {err}\n")); }
    if let Some(err) = capture_error { out.push_str(&format!("Capture error: {err}\n")); }
    if let Some(dir) = temp_dir { out.push_str(&format!("Working directory: {}\n", dir.to_string_lossy())); }
    let log = recent_recorder_log();
    if !log.is_empty() { out.push_str("\nRecent recorder log:\n"); out.push_str(&log); out.push('\n'); }
    Ok(redact_diagnostic_paths(&out))
}

#[tauri::command]
pub fn get_unsaved_recording(state:tauri::State<'_,RecorderState>)->Result<Option<String>,String>{
    Ok(state.inner.lock().map_err(|e|e.to_string())?.unsaved_recording.as_ref().map(|p|p.to_string_lossy().to_string()))
}

#[tauri::command]
pub fn save_unsaved_recording(state:tauri::State<'_,RecorderState>,destination:String)->Result<String,String>{
    let mut i=state.inner.lock().map_err(|e|e.to_string())?;
    let src=i.unsaved_recording.clone().ok_or("There is no unsaved screen recording.")?;
    let dst=PathBuf::from(destination);
    if let Some(parent)=dst.parent(){fs::create_dir_all(parent).map_err(|e|e.to_string())?;}
    fs::copy(&src,&dst).map_err(|e|format!("failed to save original recording: {e}"))?;
    if let Some(dir)=&i.unsaved_dir { let _=fs::write(dir.join(".saved"), b"saved"); }
    i.unsaved_recording=None;
    Ok(dst.to_string_lossy().to_string())
}

#[tauri::command]
pub fn acknowledge_exported_recording(state:tauri::State<'_,RecorderState>)->Result<(),String>{
    let mut i=state.inner.lock().map_err(|e|e.to_string())?;
    if i.unsaved_recording.is_none() { return Ok(()); }
    // The editor exported the untouched full recording to a user-chosen path.
    // Mark this temp session as settled but keep its working directory alive
    // while Chromium is still previewing the source. It is removed on the
    // next recorder launch or normal app shutdown. The marker also prevents
    // crash recovery from resurrecting an already-saved take.
    if let Some(dir)=&i.unsaved_dir {
        fs::write(dir.join(".saved"), b"exported").map_err(|e|format!("could not mark recording as saved: {e}"))?;
    }
    i.unsaved_recording=None;
    Ok(())
}

#[tauri::command]
pub fn discard_unsaved_recording(state:tauri::State<'_,RecorderState>)->Result<(),String>{
    let dir={state.inner.lock().map_err(|e|e.to_string())?.unsaved_dir.clone()};
    if let Some(d)=&dir {
        match fs::remove_dir_all(d) {
            Ok(()) => {},
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("could not delete temporary recording: {e}")),
        }
    }
    let mut i=state.inner.lock().map_err(|e|e.to_string())?;
    i.unsaved_dir=None; i.unsaved_recording=None;
    Ok(())
}


/// Best-effort crash recovery discovery. Finalized MKV recordings are
/// preferred; otherwise the newest finalized native MP4 segment from an
/// interrupted session is surfaced instead of being abandoned in %TEMP%.
#[tauri::command]
pub fn find_recoverable_recording(state: tauri::State<'_, RecorderState>) -> Result<Option<String>, String> {
    if state.inner.lock().map_err(|e| e.to_string())?.unsaved_recording.is_some() {
        return Ok(None);
    }
    let root = recorder_root();
    if !root.exists() { return Ok(None); }
    let mut candidates: Vec<(SystemTime, PathBuf, PathBuf)> = vec![];
    for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let dir = entry.path();
        if !dir.is_dir() { continue; }
        if dir.join(".saved").exists() { let _=fs::remove_dir_all(&dir); continue; }
        let mut finals = vec![];
        let mut segs = vec![];
        if let Ok(rd) = fs::read_dir(&dir) {
            for f in rd.flatten() {
                let p = f.path();
                let ext=p.extension().and_then(|x|x.to_str()).unwrap_or("");
                let name=p.file_name().and_then(|x|x.to_str()).unwrap_or("");
                if ext.eq_ignore_ascii_case("mkv") && name.starts_with("Klippit-Recording-") {
                    finals.push(p);
                } else if ext.eq_ignore_ascii_case("mp4") && name.starts_with("segment-") {
                    // Native paused/stopped segments are finalized MP4 files and
                    // can be reopened after an interrupted session. An MP4 that
                    // was still being written at the moment of a hard crash may
                    // not be recoverable, so ffprobe/editor validation still has
                    // the final say when the user reopens it.
                    segs.push(p);
                }
            }
        }
        let picked = finals.into_iter().max_by_key(|p| fs::metadata(p).and_then(|m|m.modified()).unwrap_or(UNIX_EPOCH))
            .or_else(|| segs.into_iter().max_by_key(|p| fs::metadata(p).map(|m|m.len()).unwrap_or(0)));
        if let Some(p)=picked {
            if fs::metadata(&p).map(|m|m.len()>1024).unwrap_or(false) {
                let t=fs::metadata(&p).and_then(|m|m.modified()).unwrap_or(UNIX_EPOCH);
                candidates.push((t,p,dir.clone()));
            }
        }
    }
    if let Some((_t,p,d))=candidates.into_iter().max_by_key(|x|x.0) {
        let mut i=state.inner.lock().map_err(|e|e.to_string())?;
        i.unsaved_recording=Some(p.clone()); i.unsaved_dir=Some(d);
        return Ok(Some(p.to_string_lossy().to_string()));
    }
    Ok(None)
}

#[tauri::command]
pub fn reopen_unsaved_recording(app: AppHandle, state: tauri::State<'_, RecorderState>) -> Result<String,String> {
    let path=state.inner.lock().map_err(|e|e.to_string())?.unsaved_recording.clone().ok_or("No recoverable recording found.")?;
    if let Some(main)=app.get_webview_window("main") {
        let normalized=path.to_string_lossy().replace('\\',"/");
        let file=path.file_name().and_then(|s|s.to_str()).unwrap_or("Klippit Recording");
        let init=serde_json::json!({"filePath":normalized,"fileName":file,"startTime":0,"subtitle":{"available":false},"temporaryRecording":true});
        let _=main.eval(&format!("window.applyInit && window.applyInit({});",init));
        let _=main.show(); let _=main.set_focus();
    }
    Ok(path.to_string_lossy().to_string())
}

pub fn emit_hotkey(app:&AppHandle,action:&str){ let _=app.emit_to("recorder-controls","recorder-hotkey",action); }
