// Klippit — panel logic
//
// Mirrors sakuga-enhancer.js's frame-bar pattern: every nudge, whether from
// a mouse click, a dragged handle, or a keypress, funnels through one
// step()/setHandleTime() path. There is no separate "keyboard mode" and
// "mouse mode" — same state, same function, three input paths.
'use strict';

// ---------- state ----------
// mpv's handoff (see mpv-scripts/clip-trigger.lua) still needs to inject
// window.__KLIPPIT_INIT__ before this script runs — that wiring (reading
// --init on the Rust side, evaluating it into the webview pre-load) isn't
// done yet, so init below still falls back to dev-preview values whether
// or not you launched this from mpv. Metadata/export/dialog/close calls
// below, on the other hand, are wired to the real Tauri backend.
var init = window.__KLIPPIT_INIT__ || {
  filePath: '',
  fileName: '(no file — dev preview mode)',
  startTime: 12.0,
  subtitle: { available: false }
};

var state = {
  duration: 0,
  fps: 24, // replaced with the real value once ffprobe reports it (see loadMetadata)
  inTime: init.startTime,
  outTime: init.startTime + 3,
  activeHandle: 'in', // 'in' | 'out' — which one the frame bar and , / . keys nudge
  format: 'mp4', // 'mp4' | 'gif'
  burnSubs: false,
  mode: 'quality', // 'quality' | 'size'
  crf: 23,
  resolution: 0, // 0 = source
  gifFps: 24,
  targetMb: 10,
  outputDir: '~/Videos/Clips',
  outputFileName: '', // populated once we know the source filename — see updateDefaultFilename()
  sourceWidth: 0,
  sourceHeight: 0
};

var MED_STEP = 5;

// ---------- dom ----------
var video = document.getElementById('preview');
var seekBar = document.getElementById('seek-bar');
var trimTrack = document.getElementById('trim-track');
var trimRange = document.getElementById('trim-range');
var handleIn = document.getElementById('handle-in');
var handleOut = document.getElementById('handle-out');
var selIn = document.getElementById('sel-in');
var selOut = document.getElementById('sel-out');
var frameCount = document.getElementById('frame-count');
var frameTime = document.getElementById('frame-time');
var statusEl = document.getElementById('status');

// ---------- helpers ----------
function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }

function fmtTime(t) {
  var m = Math.floor(t / 60);
  var s = (t - m * 60).toFixed(2);
  return m + ':' + (s < 10 ? '0' : '') + s;
}

function frameOf(t) { return Math.round(t * state.fps); }
function timeOfFrame(f) { return f / state.fps; }

// ---------- rendering ----------
function render() {
  var dur = state.duration || 1;
  var inPct = (state.inTime / dur) * 100;
  var outPct = (state.outTime / dur) * 100;
  handleIn.style.left = inPct + '%';
  handleOut.style.left = outPct + '%';
  trimRange.style.left = inPct + '%';
  trimRange.style.width = Math.max(0, outPct - inPct) + '%';

  var activeTime = state.activeHandle === 'in' ? state.inTime : state.outTime;
  frameCount.textContent = frameOf(activeTime) + ' / ' + frameOf(dur) +
    ' (' + state.activeHandle + ')';
  frameTime.textContent =
    'in ' + fmtTime(state.inTime) + '  \u2192  out ' + fmtTime(state.outTime) +
    '  (' + fmtTime(Math.max(0, state.outTime - state.inTime)) + ')';

  selIn.setAttribute('aria-pressed', state.activeHandle === 'in');
  selOut.setAttribute('aria-pressed', state.activeHandle === 'out');
  updateDefaultFilename();
}

// ---------- output filename ----------
// Defaults to sourceName_in-out, editable freely — once the person types
// anything in the field, we stop overwriting it as in/out change, so
// their edit sticks rather than getting silently clobbered on the next
// handle move.
var filenameManuallyEdited = false;
function fileStem(name) {
  var idx = name.lastIndexOf('.');
  return idx > 0 ? name.slice(0, idx) : name;
}
function updateDefaultFilename() {
  if (filenameManuallyEdited) return;
  var stem = (init.fileName && init.fileName.indexOf('(no file') !== 0) ? fileStem(init.fileName) : 'clip';
  state.outputFileName = stem + '_' + state.inTime.toFixed(2) + '-' + state.outTime.toFixed(2);
  var field = document.getElementById('output-filename');
  if (field) field.value = state.outputFileName;
}
function updateOutputExt() {
  document.getElementById('output-ext').textContent = '.' + state.format;
}
document.getElementById('output-filename').addEventListener('input', function (e) {
  filenameManuallyEdited = true;
  state.outputFileName = e.target.value;
});

// ---------- shared step function (mouse buttons AND keyboard both call this) ----------
function step(deltaFrames) {
  var key = state.activeHandle === 'in' ? 'inTime' : 'outTime';
  var currentFrame = frameOf(state[key]);
  var next = clamp(currentFrame + deltaFrames, 0, frameOf(state.duration));
  setHandleTime(state.activeHandle, timeOfFrame(next));
}

function setHandleTime(which, t, skipSeek) {
  t = clamp(t, 0, state.duration);
  // No longer clamped against the other handle's position (see the real
  // bug this caused: In defaults to armed, Out starts just 3s after In,
  // so any scrub/seek past 3s got silently pinned back to ~3s until Out
  // was moved somewhere real first — looked exactly like "seeking doesn't
  // work until you've clicked something," which is what it was). Export
  // already validates in < out at export time, so the UI doesn't need to
  // enforce it live too — doing so only got in the way of free scrubbing.
  if (which === 'in') {
    state.inTime = t;
  } else {
    state.outTime = t;
  }
  // Scrub the preview to whichever point was just touched, so clicking a
  // frame-step button doubles as a visual check of where you landed.
  // skipSeek is set by the timeupdate listener below, where
  // video.currentTime is already the source of the change — re-assigning
  // it there would just interrupt playback with a redundant seek.
  if (!skipSeek) {
    video.currentTime = which === 'in' ? state.inTime : state.outTime;
  }
  render();
}

// ---------- frame bar: mouse ----------
document.getElementById('fb-back').onclick = function () { step(-1); };
document.getElementById('fb-fwd').onclick = function () { step(1); };
document.getElementById('fb-medback').onclick = function () { step(-MED_STEP); };
document.getElementById('fb-medfwd').onclick = function () { step(MED_STEP); };
document.getElementById('fb-bigback').onclick = function () { step(-Math.round(state.fps)); };
document.getElementById('fb-bigfwd').onclick = function () { step(Math.round(state.fps)); };

selIn.onclick = function () { state.activeHandle = 'in'; video.currentTime = state.inTime; render(); };
selOut.onclick = function () { state.activeHandle = 'out'; video.currentTime = state.outTime; render(); };

// ---------- frame bar: keyboard ----------
// Same , / . convention as sakuga-enhancer.js, so the muscle memory carries
// over. Shift+,/. mirrors the medium-step buttons; the big ~1s jump is
// deliberately mouse/button-only (« / ») since it's a coarse repositioning
// move, not something worth a dedicated key.
window.addEventListener('keydown', function (e) {
  // Don't hijack typing in the target-MB, filename, or other text fields.
  if (e.target.tagName === 'INPUT' && e.target.type !== 'range') return;

  if (e.key === ',') { step(e.shiftKey ? -MED_STEP : -1); e.preventDefault(); }
  else if (e.key === '.') { step(e.shiftKey ? MED_STEP : 1); e.preventDefault(); }
  else if (e.key === 'Enter' && e.target.tagName !== 'BUTTON') { exportClip(); }
  else if (e.key === 'Escape') { closePanel(); }
  else if (e.key === ' ' && e.target.tagName !== 'BUTTON') { togglePlayback(); e.preventDefault(); }
});

// ---------- play/pause (editing preview) ----------
var playPauseBtn = document.getElementById('play-pause-btn');
function togglePlayback() {
  if (video.paused) { video.play(); } else { video.pause(); }
}
playPauseBtn.onclick = togglePlayback;
video.addEventListener('play', function () { playPauseBtn.innerHTML = '&#10074;&#10074;'; });
video.addEventListener('pause', function () { playPauseBtn.innerHTML = '&#9654;'; });

// ---------- subtitle preview visibility (separate from export burn-in) ----------
// A distinct control from the Subtitles Off/Burn-in toggle in the
// sidebar (which governs the FINAL EXPORT) — this one governs whether
// you currently SEE the overlay while editing, and doubles as what
// screenshots capture: "screenshot shows what you're currently looking
// at" is the more intuitive tie-in than reusing the export setting,
// which is a separate decision about the eventual clip.
var subsPreviewVisible = true;
var subsPreviewToggleBtn = document.getElementById('subs-preview-toggle');
subsPreviewToggleBtn.onclick = function () {
  subsPreviewVisible = !subsPreviewVisible;
  subsPreviewToggleBtn.setAttribute('aria-pressed', String(subsPreviewVisible));
  applySubsPreviewVisibility();
};
function applySubsPreviewVisibility() {
  if (octopusInstance && octopusInstance.canvas) {
    octopusInstance.canvas.style.display = subsPreviewVisible ? '' : 'none';
  }
}

// Small accessors rather than duplicating this into separate state —
// init.subtitle is already the single source of truth, refreshed
// correctly on every file switch via applyInit().
function currentSubtitleLang() {
  return (init.subtitle && init.subtitle.lang) || null;
}
function currentSubtitleExternalFile() {
  return (init.subtitle && init.subtitle.externalFile) || null;
}

// ---------- screenshot ----------
// Uses the live preview visibility toggle above, not the export burn-in
// setting — "with or without subtitles" for a screenshot is answered by
// whatever you're currently seeing on screen, independent of whatever
// you've decided for the full clip's export.
var screenshotBtn = document.getElementById('screenshot-btn');
screenshotBtn.onclick = function () {
  if (!init.filePath || !window.__TAURI__) {
    setStatus('screenshot needs a real file + Tauri backend (not available in dev preview)', 'error');
    return;
  }
  var stem = (init.fileName && init.fileName.indexOf('(no file') !== 0) ? fileStem(init.fileName) : 'clip';
  var outPath = state.outputDir.replace(/[\\/]+$/, '') + '/' + stem + '_' + video.currentTime.toFixed(2) + '.png';

  setStatus('capturing screenshot…', 'busy');
  window.__TAURI__.core.invoke('extract_frame', {
    path: init.filePath,
    at: video.currentTime,
    burnSubs: subsPreviewVisible,
    outPath: outPath,
    sourceWidth: state.sourceWidth,
    sourceHeight: state.sourceHeight,
    subtitleLang: currentSubtitleLang(),
    subtitleExternalFile: currentSubtitleExternalFile()
  }).then(function (path) {
    setStatus('screenshot saved — ' + path, 'done');
  }).catch(function (err) {
    setStatus('screenshot failed: ' + err, 'error');
  });
};

// ---------- draggable trim handles (mouse) ----------
function makeDraggable(handle, which) {
  handle.addEventListener('pointerdown', function (e) {
    handle.setPointerCapture(e.pointerId);
    state.activeHandle = which;
    render();
    function onMove(ev) {
      var rect = trimTrack.getBoundingClientRect();
      var pct = clamp((ev.clientX - rect.left) / rect.width, 0, 1);
      setHandleTime(which, pct * state.duration);
    }
    function onUp() {
      handle.removeEventListener('pointermove', onMove);
      handle.removeEventListener('pointerup', onUp);
    }
    handle.addEventListener('pointermove', onMove);
    handle.addEventListener('pointerup', onUp);
  });
  // Arrow-key nudging when a handle itself has focus, as an alternative to
  // the , / . convention — both land on the same setHandleTime call.
  handle.addEventListener('keydown', function (e) {
    if (e.key === 'ArrowLeft') { state.activeHandle = which; step(-1); e.preventDefault(); }
    else if (e.key === 'ArrowRight') { state.activeHandle = which; step(1); e.preventDefault(); }
  });
}
makeDraggable(handleIn, 'in');
makeDraggable(handleOut, 'out');

// ---------- seek bar (whole-file scrub, independent of trim range) ----------
seekBar.addEventListener('input', function () {
  var pct = seekBar.value / 1000;
  var t = pct * state.duration;
  video.currentTime = t;
  // Drive the armed-handle sync directly here rather than relying solely
  // on the video's 'timeupdate' event below — for a paused, discrete seek
  // (as opposed to continuous playback), browsers don't reliably fire
  // 'timeupdate' for every rapid intermediate position during a drag, so
  // depending on it alone meant dragging this bar could silently fail to
  // move the trim handle until something else (like clicking In/Out)
  // happened to trigger a sync some other way.
  setHandleTime(state.activeHandle, t, true);
});
video.addEventListener('timeupdate', function () {
  if (state.duration) seekBar.value = (video.currentTime / state.duration) * 1000;

  var t = video.currentTime;
  // During unattended *playback* specifically (not deliberate scrubbing,
  // which stays completely free — see setHandleTime), auto-pause right
  // when the armed handle's forward drift would cross the other,
  // already-meaningfully-set point, like a preview loop stopping at its
  // boundary. Without this, In (the default-armed handle) just drifts
  // forever past a real Out point during hands-off playback, leaving you
  // to notice and manually walk it back afterward.
  if (!video.paused && state.activeHandle === 'in' && state.outTime > state.inTime && t >= state.outTime) {
    video.pause();
    setHandleTime('in', state.outTime, false);
    return;
  }
  if (!video.paused && state.activeHandle === 'out' && state.inTime < state.outTime && t <= state.inTime) {
    video.pause();
    setHandleTime('out', state.inTime, false);
    return;
  }

  // Whichever handle is armed (In or Out) follows video.currentTime for
  // any reason it changes — playback advancing, dragging the seek bar,
  // frame-stepping, or dragging a trim handle. This is what makes
  // pressing Play actually move the armed point instead of just playing
  // disconnected from editing: press Play, watch, press Pause/Space
  // right when you want that point, and it's already set.
  setHandleTime(state.activeHandle, t, true);
});

// ---------- format / subtitle / mode toggles ----------
function bindSeg(idOn, idOff, onSelect) {
  var a = document.getElementById(idOn), b = document.getElementById(idOff);
  a.onclick = function () { a.setAttribute('aria-pressed', 'true'); b.setAttribute('aria-pressed', 'false'); onSelect(idOn); };
  b.onclick = function () { b.setAttribute('aria-pressed', 'true'); a.setAttribute('aria-pressed', 'false'); onSelect(idOff); };
}
bindSeg('fmt-mp4', 'fmt-gif', function (id) {
  state.format = id === 'fmt-mp4' ? 'mp4' : 'gif';
  document.getElementById('fps-field').style.display = state.format === 'gif' ? 'block' : 'none';
  updateOutputExt();
});
bindSeg('mode-quality', 'mode-size', function (id) {
  state.mode = id === 'mode-quality' ? 'quality' : 'size';
  document.getElementById('panel-quality').style.display = state.mode === 'quality' ? 'block' : 'none';
  document.getElementById('panel-size').style.display = state.mode === 'size' ? 'block' : 'none';
});

bindSeg('subs-off', 'subs-on', function (id) {
  state.burnSubs = id === 'subs-on';
});
var subsOffBtn = document.getElementById('subs-off');
var subsOnBtn = document.getElementById('subs-on');
var subsStatus = document.getElementById('subs-status');
function disableSubtitleControls(message) {
  subsOffBtn.disabled = true;
  subsOnBtn.disabled = true;
  subsStatus.textContent = message;
}
// A curt, VLC-specific message when there's no embedded subtitle and the
// trigger came from VLC specifically — external subtitle files loaded in
// VLC ("Add Subtitle File") aren't detectable (see the VLC scripts'
// comments for why an earlier attempt at guessing this was dropped as
// too speculative), so it's worth being upfront about rather than just
// showing the generic "no subtitle stream" message, which reads as if
// Klippit failed to notice something that's actually just unsupported.
function noSubtitleMessage() {
  return init.trigger === 'vlc'
    ? 'No embedded sub detected. External sub handling not supported on VLC.'
    : 'No subtitle stream detected';
}

// ---------- size presets ----------
Array.prototype.forEach.call(document.querySelectorAll('.preset-btn'), function (btn) {
  btn.onclick = function () {
    Array.prototype.forEach.call(document.querySelectorAll('.preset-btn'), function (b) { b.setAttribute('aria-pressed', 'false'); });
    btn.setAttribute('aria-pressed', 'true');
    var mb = parseFloat(btn.getAttribute('data-mb'));
    state.targetMb = mb;
    document.getElementById('target-mb').value = mb;
  };
});
document.getElementById('target-mb').addEventListener('input', function (e) {
  state.targetMb = parseFloat(e.target.value) || 0;
  Array.prototype.forEach.call(document.querySelectorAll('.preset-btn'), function (b) { b.setAttribute('aria-pressed', 'false'); });
});
document.getElementById('crf').addEventListener('input', function (e) { state.crf = parseInt(e.target.value, 10); });
document.getElementById('res').addEventListener('change', function (e) { state.resolution = parseInt(e.target.value, 10); });
document.getElementById('fps').addEventListener('change', function (e) { state.gifFps = parseInt(e.target.value, 10); });

// ---------- output location ----------
document.getElementById('browse-btn').onclick = function () {
  if (!window.__TAURI__) {
    // Standalone browser preview — no Tauri backend to open a native dialog.
    var dir = prompt('Output folder (dev preview — no Tauri backend here):', state.outputDir);
    if (dir) { state.outputDir = dir; document.getElementById('output-path').textContent = dir; }
    return;
  }
  window.__TAURI__.dialog.open({ directory: true, defaultPath: state.outputDir }).then(function (dir) {
    if (dir) { state.outputDir = dir; document.getElementById('output-path').textContent = dir; }
  }).catch(function (err) { setStatus('folder picker failed: ' + err, 'error'); });
};

// ---------- metadata + video loading ----------
function loadMetadata() {
  if (!init.filePath || !window.__TAURI__) return; // dev preview mode, no real source / no backend
  window.__TAURI__.core.invoke('get_video_metadata', { path: init.filePath }).then(function (meta) {
    state.duration = meta.duration;
    state.fps = meta.fps || state.fps;
    state.sourceWidth = meta.width || 0;
    state.sourceHeight = meta.height || 0;
    if (!meta.hasSubtitles) {
      disableSubtitleControls(noSubtitleMessage());
      disposeSubtitleOverlay();
    } else {
      // This ffprobe-based check is the reliable signal — re-enable
      // regardless of what mpv/VLC's active-track state guessed earlier
      // in applyInit(), since that only reflects whether a track was
      // actively selected in the player, not whether the file has one.
      subsOffBtn.disabled = false;
      subsOnBtn.disabled = false;
      subsStatus.textContent = '';
      loadSubtitleOverlay();
    }
    render();
  }).catch(function (err) {
    setStatus('failed to read video metadata: ' + err, 'error');
  });
}

// ---------- subtitle preview overlay (libass-wasm) ----------
// A plain <video> element cannot render embedded ASS/SSA tracks from an
// MKV at all — not a bug, Chromium's media pipeline just doesn't support
// it. subtitles-octopus.js (libass compiled to WebAssembly, vendored
// under src/lib/) renders them as a separate overlay synced to the same
// video element instead. Shown whenever the source has subtitles,
// independent of the export burn-in toggle — seeing dialogue timing is
// useful for trimming regardless of whether you plan to burn subs into
// the final export.
var octopusInstance = null;
function disposeSubtitleOverlay() {
  if (octopusInstance) {
    try { octopusInstance.dispose(); } catch (e) { /* already gone */ }
    octopusInstance = null;
  }
}
function loadSubtitleOverlay() {
  disposeSubtitleOverlay();
  window.__TAURI__.core.invoke('extract_subtitles_for_preview', {
    path: init.filePath,
    subtitleLang: currentSubtitleLang(),
    subtitleExternalFile: currentSubtitleExternalFile()
  }).then(function (data) {
    var convert = window.__TAURI__.core.convertFileSrc;

    // Fetch on the MAIN thread (proven to work — same mechanism the video
    // element itself already uses successfully) and hand the worker
    // plain blob: URLs instead of Tauri's own asset-protocol URLs
    // directly. Confirmed via a real "Loading data file ... failed"
    // error that the worker can't reliably fetch those custom-scheme
    // URLs itself, even once correctly formed — a known category of
    // issue with Worker-based libraries in Tauri/Electron-style apps.
    // blob: URLs are a standard mechanism any context can resolve,
    // sidestepping that entirely.
    function toBlobUrl(path) {
      return fetch(convert(path))
        .then(function (r) { return r.blob(); })
        .then(function (b) { return URL.createObjectURL(b); });
    }

    Promise.all([
      toBlobUrl(data.assPath),
      Promise.all(data.fontPaths.map(toBlobUrl))
    ]).then(function (results) {
      octopusInstance = new SubtitlesOctopus({
        video: video,
        subUrl: results[0],
        fonts: results[1],
        workerUrl: 'lib/subtitles-octopus/subtitles-octopus-worker.js',
        onReady: function () {
          // Apply the current preview-visibility toggle to this freshly
          // created instance (e.g. after switching files) — onReady is
          // used rather than assuming canvas exists immediately after
          // construction, since the library's setup may finish async.
          applySubsPreviewVisibility();
        },
        onError: function (err) {
          console.log('[klippit] subtitle overlay error:', err);
        }
      });
      applySubsPreviewVisibility(); // harmless if canvas isn't ready yet — onReady above covers that case
    }).catch(function (err) {
      console.log('[klippit] failed to prepare subtitle blob URLs:', err);
    });
  }).catch(function (err) {
    // Non-fatal — editing still works fine without the subtitle overlay,
    // this just means you won't see dialogue timing while trimming.
    console.log('[klippit] subtitle preview extraction failed:', err);
  });
}

function setStatus(text, kind) {
  statusEl.textContent = text;
  statusEl.className = kind || '';
}

var spinnerEl = document.getElementById('export-spinner');
var exportBtn = document.getElementById('export-btn');
var statusActions = document.getElementById('status-actions');
var revealBtn = document.getElementById('reveal-btn');
var playBtn = document.getElementById('play-btn');
var lastExportedPath = null;

function setBusy(busy) {
  exportBtn.disabled = busy;
  spinnerEl.style.display = busy ? 'inline-block' : 'none';
}

// ---------- export ----------
function exportClip() {
  var params = {
    filePath: init.filePath,
    inTime: state.inTime,
    outTime: state.outTime,
    format: state.format,
    burnSubs: state.burnSubs,
    mode: state.mode,
    crf: state.crf,
    resolution: state.resolution,
    gifFps: state.gifFps,
    targetMb: state.targetMb,
    outputDir: state.outputDir,
    fileName: state.outputFileName,
    sourceWidth: state.sourceWidth,
    sourceHeight: state.sourceHeight,
    subtitleLang: currentSubtitleLang(),
    subtitleExternalFile: currentSubtitleExternalFile()
  };
  setStatus('exporting…', 'busy');
  statusActions.style.display = 'none';
  setBusy(true);

  if (!window.__TAURI__) {
    // Standalone browser preview — no backend to actually encode against.
    console.log('[klippit] export_clip params (dev preview, no backend):', params);
    setTimeout(function () {
      setStatus('(dev preview) no Tauri backend here — see console for params', 'done');
      setBusy(false);
    }, 400);
    return;
  }

  window.__TAURI__.core.invoke('export_clip', { params: params }).then(function (path) {
    setStatus('done — ' + path, 'done');
    lastExportedPath = path;
    statusActions.style.display = 'flex';
  }).catch(function (err) {
    setStatus('export failed: ' + err, 'error');
  }).then(function () {
    setBusy(false);
  });
}
document.getElementById('export-btn').onclick = exportClip;
document.getElementById('cancel-btn').onclick = closePanel;

revealBtn.onclick = function () {
  if (!lastExportedPath || !window.__TAURI__) return;
  // TODO(verify): tauri-plugin-opener's exact JS export names — written
  // against the documented v2 API (openPath / revealItemInDir under
  // window.__TAURI__.opener) but not yet confirmed against a real build.
  window.__TAURI__.opener.revealItemInDir(lastExportedPath).catch(function (err) {
    setStatus('failed to open folder: ' + err, 'error');
  });
};
playBtn.onclick = function () {
  if (!lastExportedPath || !window.__TAURI__) return;
  // Opens a small dedicated review window (review.html) with a native
  // <video controls> player, rather than handing off to the OS default
  // app — keeps the review experience inside Klippit itself.
  window.__TAURI__.core.invoke('open_review_window', { path: lastExportedPath }).catch(function (err) {
    setStatus('failed to open preview: ' + err, 'error');
  });
};

function closePanel() {
  if (window.__TAURI__) {
    window.__TAURI__.window.getCurrentWindow().close();
  } else {
    console.log('[klippit] close panel (dev preview, no window to close)');
  }
}

// ---------- apply init (first launch, and later re-seeds from single-instance) ----------
// Pressing mpv's trigger key while a Klippit window is already open
// doesn't spawn a second one — see main.rs's single-instance plugin
// handler, which calls window.applyInit(...) directly via eval() on the
// existing window instead. Wrapping this in a named, re-callable
// function (rather than one-shot top-level code) is what makes that
// possible.
function applyInit(newInit) {
  init = newInit || init;
  document.getElementById('source-name').textContent = init.fileName;
  document.getElementById('source-name').title = init.filePath;

  // Reset editing state for the new clip rather than carrying over
  // whatever in/out/filename the previous file had.
  state.inTime = init.startTime;
  state.outTime = init.startTime + 3;
  state.activeHandle = 'in';
  filenameManuallyEdited = false;
  lastExportedPath = null;
  state.sourceWidth = 0;
  state.sourceHeight = 0;
  statusActions.style.display = 'none';
  setStatus('', '');
  disposeSubtitleOverlay(); // old file's overlay shouldn't linger over the new video

  // Neutral starting state — loadMetadata() below is the sole authority
  // on whether this file actually has subtitles (a real ffprobe check),
  // so nothing here should pre-emptively disable anything based on
  // mpv/VLC's active-track state, which only reflects what was selected
  // in the player, not what the file contains.
  subsOffBtn.disabled = false;
  subsOnBtn.disabled = false;
  subsStatus.textContent = '';

  if (init.filePath) {
    video.src = window.__TAURI__
      ? window.__TAURI__.core.convertFileSrc(init.filePath)
      : 'file://' + init.filePath; // browser dev-preview fallback only
    video.load();
  }

  loadMetadata();
  render();
}
// Exposed globally so main.rs's single-instance handler can call this
// directly on the already-open window via window.eval(...).
window.applyInit = applyInit;

video.addEventListener('loadedmetadata', function () {
  state.duration = video.duration || 0;
  video.currentTime = state.inTime;
  render();
});

updateOutputExt();
applyInit(init);
