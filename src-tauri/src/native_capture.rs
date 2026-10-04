//! Native Windows screen-capture pipeline used by Klippit's recorder.
//!
//! The important architectural rule here is that capture is prepared when the
//! recorder opens. Pressing Record does not spawn FFmpeg or create a new screen
//! capture source. A long-lived Windows Graphics Capture session keeps the most
//! recent frame available, while a separate encoder thread writes frames at the
//! requested fixed cadence and duplicates the latest frame when the desktop is
//! unchanged.

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
    VideoSettingsSubType,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRegion {
    fn end_x(self) -> u32 { self.x.saturating_add(self.width) }
    fn end_y(self) -> u32 { self.y.saturating_add(self.height) }
}

#[derive(Debug, Default, Clone)]
pub struct NativeStatsSnapshot {
    pub fps: f64,
    pub frame_count: u64,
    pub dropped_frames: u64,
    pub duplicated_frames: u64,
    pub capture_updates: u64,
    pub error: Option<String>,
}

#[derive(Debug)]
struct FrameStore {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    generation: u64,
    region: Option<CaptureRegion>,
}

impl Default for FrameStore {
    fn default() -> Self {
        Self { bytes: Vec::new(), width: 0, height: 0, generation: 0, region: None }
    }
}

#[derive(Debug, Default)]
struct CaptureShared {
    // While idle we keep a packed top-to-bottom snapshot of the full monitor.
    // It lets Start immediately crop an initial frame even if the desktop is
    // completely static at the moment the user presses Record.
    idle_full: Mutex<FrameStore>,
    // While recording this contains the selected region in bottom-to-top BGRA,
    // which is the layout expected by VideoEncoder::send_frame_buffer.
    latest_region: Mutex<FrameStore>,
    recording_region: Mutex<Option<CaptureRegion>>,
    ready: AtomicBool,
    capture_updates: AtomicU64,
    last_error: Mutex<Option<String>>,
}

#[derive(Clone)]
struct CaptureFlags {
    shared: Arc<CaptureShared>,
}

struct LatestFrameHandler {
    shared: Arc<CaptureShared>,
    padding_scratch: Vec<u8>,
}

impl LatestFrameHandler {
    fn copy_top_down(slot: &mut FrameStore, packed: &[u8], width: u32, height: u32) {
        let expected = width as usize * height as usize * 4;
        slot.bytes.resize(expected, 0);
        if packed.len() >= expected {
            slot.bytes.copy_from_slice(&packed[..expected]);
        }
        slot.width = width;
        slot.height = height;
        slot.region = None;
        slot.generation = slot.generation.wrapping_add(1);
    }

    fn copy_bottom_up(
        slot: &mut FrameStore,
        packed: &[u8],
        width: u32,
        height: u32,
        region: CaptureRegion,
    ) {
        let row = width as usize * 4;
        let expected = row * height as usize;
        slot.bytes.resize(expected, 0);
        if packed.len() >= expected {
            for dst_y in 0..height as usize {
                let src_y = height as usize - 1 - dst_y;
                let src = &packed[src_y * row..src_y * row + row];
                slot.bytes[dst_y * row..dst_y * row + row].copy_from_slice(src);
            }
        }
        slot.width = width;
        slot.height = height;
        slot.region = Some(region);
        slot.generation = slot.generation.wrapping_add(1);
    }
}

impl GraphicsCaptureApiHandler for LatestFrameHandler {
    type Flags = CaptureFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { shared: ctx.flags.shared, padding_scratch: Vec::new() })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame<'_>,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // Keep the shared state in a local Arc so borrowing padding_scratch for
        // the mapped frame never forces another borrow of all of `self`.
        let shared = self.shared.clone();
        let remember = |message: String| -> String {
            if let Ok(mut slot) = shared.last_error.lock() {
                *slot = Some(message.clone());
            }
            message
        };
        let region = *shared.recording_region.lock()
            .map_err(|e| remember(format!("native capture region lock failed: {e}")))?;

        if let Some(region) = region {
            if region.end_x() > frame.width() || region.end_y() > frame.height() {
                return Err(remember(format!(
                    "capture region {}x{} at {},{} is outside the {}x{} monitor frame",
                    region.width, region.height, region.x, region.y, frame.width(), frame.height()
                )));
            }
            let buffer = frame.buffer_crop(region.x, region.y, region.end_x(), region.end_y())
                .map_err(|e| remember(format!("native region readback failed: {e}")))?;
            let packed = buffer.as_nopadding_buffer(&mut self.padding_scratch);
            let mut slot = shared.latest_region.lock()
                .map_err(|e| remember(format!("native region frame lock failed: {e}")))?;
            Self::copy_bottom_up(&mut slot, packed, region.width, region.height, region);
        } else {
            let width = frame.width();
            let height = frame.height();
            let buffer = frame.buffer()
                .map_err(|e| remember(format!("native monitor readback failed: {e}")))?;
            let packed = buffer.as_nopadding_buffer(&mut self.padding_scratch);
            let mut slot = shared.idle_full.lock()
                .map_err(|e| remember(format!("native idle frame lock failed: {e}")))?;
            Self::copy_top_down(&mut slot, packed, width, height);
        }

        shared.capture_updates.fetch_add(1, Ordering::Relaxed);
        shared.ready.store(true, Ordering::Release);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        if let Ok(mut slot) = self.shared.last_error.lock() {
            *slot = Some("Windows capture session closed unexpectedly".into());
        }
        Ok(())
    }
}

pub struct NativeCaptureSession {
    monitor_name: String,
    draw_cursor: bool,
    shared: Arc<CaptureShared>,
    control: Option<CaptureControl<LatestFrameHandler, String>>,
}

impl NativeCaptureSession {
    pub fn start(monitor_name: &str, draw_cursor: bool) -> Result<Self, String> {
        let monitors = Monitor::enumerate().map_err(|e| format!("could not enumerate monitors: {e}"))?;
        let monitor = monitors.into_iter().find(|m| {
            let device_match = m.device_name()
                .map(|name| name.eq_ignore_ascii_case(monitor_name))
                .unwrap_or(false);
            let friendly_match = m.name()
                .map(|name| name.eq_ignore_ascii_case(monitor_name))
                .unwrap_or(false);
            device_match || friendly_match
        }).ok_or_else(|| format!("could not find native capture monitor {monitor_name}"))?;

        let shared = Arc::new(CaptureShared::default());

        // Several GraphicsCaptureSession properties were added in later
        // Windows releases. They are optional conveniences, not requirements
        // for basic screen capture. Never make recorder startup depend on them.
        //
        // In particular, requesting WithoutBorder on an older Windows build
        // makes windows-capture return BorderConfigUnsupported before the
        // capture session has even started. Keep the system border behavior.
        // The capture indicator is not part of the captured frame itself.
        let cursor_settings = if draw_cursor {
            // Cursor capture is enabled by default by WGC. Using Default avoids
            // touching IsCursorCaptureEnabled on OS builds that do not expose it.
            CursorCaptureSettings::Default
        } else {
            match GraphicsCaptureApi::is_cursor_settings_supported() {
                Ok(true) => CursorCaptureSettings::WithoutCursor,
                // Cursor-off is optional. On older builds, keep capture alive
                // with the system default rather than failing the whole recorder.
                Ok(false) | Err(_) => CursorCaptureSettings::Default,
            }
        };

        let settings = Settings::new(
            monitor,
            cursor_settings,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            CaptureFlags { shared: shared.clone() },
        );
        let control = LatestFrameHandler::start_free_threaded(settings)
            .map_err(|e| format!("could not initialize native Windows capture: {e}"))?;

        Ok(Self {
            monitor_name: monitor_name.to_string(),
            draw_cursor,
            shared,
            control: Some(control),
        })
    }

    pub fn matches(&self, monitor_name: &str, draw_cursor: bool) -> bool {
        self.monitor_name.eq_ignore_ascii_case(monitor_name) && self.draw_cursor == draw_cursor
    }

    pub fn is_finished(&self) -> bool {
        self.control.as_ref().map(|control| control.is_finished()).unwrap_or(true)
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.last_error.lock().ok().and_then(|v| v.clone())
    }

    pub fn is_ready(&self) -> bool { self.shared.ready.load(Ordering::Acquire) }

    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.is_ready() { return true; }
            if self.is_finished() { return false; }
            thread::sleep(Duration::from_millis(5));
        }
        self.is_ready()
    }

    fn seed_region_from_idle(&self, region: CaptureRegion) -> Result<(), String> {
        // Prefer a region frame left over from Pause if it is exactly the same
        // geometry. That makes Continue immediate even on a static desktop.
        if let Ok(slot) = self.shared.latest_region.lock() {
            if slot.region == Some(region)
                && slot.width == region.width
                && slot.height == region.height
                && slot.bytes.len() == region.width as usize * region.height as usize * 4
            {
                return Ok(());
            }
        }

        let idle = self.shared.idle_full.lock().map_err(|e| e.to_string())?;
        if idle.bytes.is_empty() || idle.width == 0 || idle.height == 0 {
            return Err("native capture is still waiting for its initial monitor frame".into());
        }
        if region.end_x() > idle.width || region.end_y() > idle.height {
            return Err(format!(
                "recording region {}x{} at {},{} is outside native monitor frame {}x{}",
                region.width, region.height, region.x, region.y, idle.width, idle.height
            ));
        }

        let src_row = idle.width as usize * 4;
        let dst_row = region.width as usize * 4;
        let mut latest = self.shared.latest_region.lock().map_err(|e| e.to_string())?;
        latest.bytes.resize(dst_row * region.height as usize, 0);
        for dst_y in 0..region.height as usize {
            // VideoEncoder's raw-buffer path expects bottom-to-top BGRA.
            let src_y = region.y as usize + (region.height as usize - 1 - dst_y);
            let src_start = src_y * src_row + region.x as usize * 4;
            let src_end = src_start + dst_row;
            latest.bytes[dst_y * dst_row..dst_y * dst_row + dst_row]
                .copy_from_slice(&idle.bytes[src_start..src_end]);
        }
        latest.width = region.width;
        latest.height = region.height;
        latest.region = Some(region);
        latest.generation = latest.generation.wrapping_add(1);
        Ok(())
    }

    pub fn start_segment(
        &self,
        output: &Path,
        region: CaptureRegion,
        fps: u32,
        codec: &str,
        bitrate: u32,
    ) -> Result<NativeSegment, String> {
        if self.is_finished() {
            return Err(self.last_error().unwrap_or_else(|| "native capture session is not running".into()));
        }
        if !self.is_ready() && !self.wait_ready(Duration::from_millis(350)) {
            return Err(self.last_error().unwrap_or_else(|| {
                "native capture did not receive its initial monitor frame within 350 ms".into()
            }));
        }

        self.seed_region_from_idle(region)?;
        {
            let mut current = self.shared.recording_region.lock().map_err(|e| e.to_string())?;
            *current = Some(region);
        }

        let subtype = match codec {
            "auto" | "h264" => VideoSettingsSubType::H264,
            "hevc" => VideoSettingsSubType::HEVC,
            "av1" => {
                if let Ok(mut current) = self.shared.recording_region.lock() { *current = None; }
                return Err("AV1 is not available in Klippit's native Windows recorder yet. Use Auto/H.264 or HEVC.".into());
            }
            _ => {
                if let Ok(mut current) = self.shared.recording_region.lock() { *current = None; }
                return Err(format!("unsupported native recorder codec: {codec}"));
            }
        };

        let settings = VideoSettingsBuilder::new(region.width, region.height)
            .sub_type(subtype)
            .bitrate(bitrate)
            .frame_rate(fps);
        let encoder = VideoEncoder::new(
            settings,
            AudioSettingsBuilder::new().disabled(true),
            ContainerSettingsBuilder::new(),
            output,
        ).map_err(|e| {
            if let Ok(mut current) = self.shared.recording_region.lock() { *current = None; }
            format!("could not initialize native Windows video encoder: {e}")
        })?;

        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let frame_count = Arc::new(AtomicU64::new(0));
        let duplicated_frames = Arc::new(AtomicU64::new(0));
        let dropped_frames = Arc::new(AtomicU64::new(0));
        let start = Instant::now();

        let shared = self.shared.clone();
        let stop2 = stop.clone();
        let finished2 = finished.clone();
        let error2 = error.clone();
        let frames2 = frame_count.clone();
        let dups2 = duplicated_frames.clone();
        let drops2 = dropped_frames.clone();
        let thread = thread::spawn(move || {
            let result = run_encoder_loop(
                encoder, shared.clone(), stop2, fps,
                frames2, dups2, drops2,
            );
            if let Err(ref e) = result {
                if let Ok(mut slot) = error2.lock() { *slot = Some(e.clone()); }
            }
            if let Ok(mut current) = shared.recording_region.lock() { *current = None; }
            finished2.store(true, Ordering::Release);
            result
        });

        let label = match codec {
            "hevc" => "Windows Media HEVC (native)",
            _ => "Windows Media H.264 (native)",
        }.to_string();

        Ok(NativeSegment {
            stop,
            finished,
            error,
            frame_count,
            duplicated_frames,
            dropped_frames,
            capture_updates_at_start: self.shared.capture_updates.load(Ordering::Relaxed),
            shared: self.shared.clone(),
            thread: Some(thread),
            started: start,
            encoder_label: label,
        })
    }

    pub fn stop(mut self) -> Result<(), String> {
        if let Some(control) = self.control.take() {
            control.stop().map_err(|e| format!("could not stop native capture session: {e}"))?;
        }
        Ok(())
    }
}

fn run_encoder_loop(
    mut encoder: VideoEncoder,
    shared: Arc<CaptureShared>,
    stop: Arc<AtomicBool>,
    fps: u32,
    frame_count: Arc<AtomicU64>,
    duplicated_frames: Arc<AtomicU64>,
    dropped_frames: Arc<AtomicU64>,
) -> Result<(), String> {
    let frame_period = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut next_tick = Instant::now();
    let mut timeline_index: u64 = 0;
    let mut encoded_count: u64 = 0;
    let mut previous_generation: Option<u64> = None;

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now < next_tick {
            thread::sleep((next_tick - now).min(Duration::from_millis(2)));
            continue;
        }

        let (generation, sent) = {
            let latest = shared.latest_region.lock().map_err(|e| e.to_string())?;
            if latest.bytes.is_empty() {
                (latest.generation, false)
            } else {
                // 100 ns units, matching WinRT TimeSpan.
                let timestamp = ((timeline_index as u128 * 10_000_000u128) / fps.max(1) as u128) as i64;
                encoder.send_frame_buffer(&latest.bytes, timestamp)
                    .map_err(|e| format!("native video encoder rejected frame {timeline_index}: {e}"))?;
                (latest.generation, true)
            }
        };

        if sent {
            if let Some(previous) = previous_generation {
                if generation == previous {
                    duplicated_frames.fetch_add(1, Ordering::Relaxed);
                } else if generation > previous.saturating_add(1) {
                    dropped_frames.fetch_add(generation - previous - 1, Ordering::Relaxed);
                }
            }
            previous_generation = Some(generation);
            timeline_index = timeline_index.saturating_add(1);
            encoded_count = encoded_count.saturating_add(1);
            frame_count.store(encoded_count, Ordering::Relaxed);
        }

        next_tick += frame_period;
        // If the process was suspended or overloaded, do not busy-loop trying
        // to manufacture an unbounded backlog. Account for missed output ticks
        // and resume from the current wall clock.
        let now = Instant::now();
        if now > next_tick + frame_period.mul_f64(2.0) {
            let behind = now.duration_since(next_tick).as_secs_f64() / frame_period.as_secs_f64();
            let skipped = behind.floor() as u64;
            if skipped > 0 {
                dropped_frames.fetch_add(skipped, Ordering::Relaxed);
                timeline_index = timeline_index.saturating_add(skipped);
                next_tick += frame_period.mul_f64(skipped as f64);
            }
        }
    }

    // Ensure an extremely short click still produces at least one video frame.
    if frame_count.load(Ordering::Relaxed) == 0 {
        let latest = shared.latest_region.lock().map_err(|e| e.to_string())?;
        if !latest.bytes.is_empty() {
            encoder.send_frame_buffer(&latest.bytes, 0)
                .map_err(|e| format!("native video encoder could not write initial frame: {e}"))?;
            frame_count.store(1, Ordering::Relaxed);
        }
    }

    encoder.finish().map_err(|e| format!("native video encoder finalization failed: {e}"))?;
    Ok(())
}

pub struct NativeSegment {
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    frame_count: Arc<AtomicU64>,
    duplicated_frames: Arc<AtomicU64>,
    dropped_frames: Arc<AtomicU64>,
    capture_updates_at_start: u64,
    shared: Arc<CaptureShared>,
    thread: Option<JoinHandle<Result<(), String>>>,
    started: Instant,
    encoder_label: String,
}

impl NativeSegment {
    pub fn encoder_label(&self) -> &str { &self.encoder_label }
    pub fn started(&self) -> Instant { self.started }
    pub fn is_finished(&self) -> bool { self.finished.load(Ordering::Acquire) }
    pub fn error(&self) -> Option<String> { self.error.lock().ok().and_then(|v| v.clone()) }

    pub fn stats(&self) -> NativeStatsSnapshot {
        let frames = self.frame_count.load(Ordering::Relaxed);
        let elapsed = self.started.elapsed().as_secs_f64();
        NativeStatsSnapshot {
            fps: if elapsed > 0.0 { frames as f64 / elapsed } else { 0.0 },
            frame_count: frames,
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            duplicated_frames: self.duplicated_frames.load(Ordering::Relaxed),
            capture_updates: self.shared.capture_updates.load(Ordering::Relaxed)
                .saturating_sub(self.capture_updates_at_start),
            error: self.error(),
        }
    }

    pub fn finish(mut self) -> Result<NativeStatsSnapshot, String> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => {},
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err("native video encoder thread crashed".into()),
            }
        }
        Ok(self.stats())
    }
}
