// Klippit — panel logic
//
// Mirrors sakuga-enhancer.js's frame-bar pattern: every nudge, whether from
// a mouse click, a dragged handle, or a keypress, funnels through one
// step()/setHandleTime() path. There is no separate "keyboard mode" and
// "mouse mode" — same state, same function, three input paths.
'use strict';

// ---------- state ----------
// The mpv/VLC handoff (see mpv-scripts/clip-trigger.lua and
// vlc-scripts/klippit-extension.lua) has been fully wired and tested for
// a long time now — window.__KLIPPIT_INIT__ gets set correctly when
// launched that way. The fallback below only matters for two genuinely
// different situations, both real: launching klippit.exe directly with
// no file context at all (a real Tauri backend, just nothing to edit
// yet), and actually opening this file in a plain browser tab with no
// Tauri runtime present (true dev preview, used for iterating on the UI
// without a full build each time). Worth keeping these worded
// differently — confirmed confusing in practice when the first case
// showed the second case's wording, reading as if something were
// broken when it wasn't.
var init = window.__KLIPPIT_INIT__ || {
  filePath: '',
  fileName: window.__TAURI__
    ? '(no file loaded — trigger from mpv or VLC)'
    : '(no file — dev preview, no Tauri backend)',
  startTime: 12.0,
  subtitle: { available: false }
};

var state = {
  duration: 0,
  fps: 24, // replaced with the real value once ffprobe reports it (see loadMetadata)
  inTime: init.startTime,
  outTime: init.startTime + 3,
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
  sourceHeight: 0,
  muteAudio: false,
  useGpu: false,
  cropEnabled: false,
  cropAspect: 'free', // 'free' | '16:9' | '9:16' | '1:1'
  // Crop rectangle in SOURCE VIDEO PIXEL coordinates, not screen pixels —
  // this is what actually gets sent to export, and stays correct
  // regardless of window resizing or preview letterboxing changes.
  cropX: 0,
  cropY: 0,
  cropWidth: 0,  // 0 = not yet initialized; set to the full frame when crop is first enabled
  cropHeight: 0
};

var MED_STEP = 5;

// ---------- dom ----------
var video = document.getElementById('preview');
var seekBar = document.getElementById('seek-bar');
var trimRange = document.getElementById('trim-range');
var handleIn = document.getElementById('handle-in');
var handleOut = document.getElementById('handle-out');
var selIn = document.getElementById('sel-in');
var selOut = document.getElementById('sel-out');
var frameCount = document.getElementById('frame-count');
var frameTime = document.getElementById('frame-time');
var statusEl = document.getElementById('status');
var previewWrap = document.getElementById('preview-wrap');

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

  updateFrameCounter();
  frameTime.textContent =
    'in ' + fmtTime(state.inTime) + '  \u2192  out ' + fmtTime(state.outTime) +
    '  (' + fmtTime(Math.max(0, state.outTime - state.inTime)) + ')';

  updateDefaultFilename();
}
// Reflects the video's current playhead position — not a marked point —
// since frame-stepping is now pure navigation (see step() below). Called
// on every timeupdate too, so it also tracks live during playback and
// scrubbing, not just after a discrete frame-step click.
function updateFrameCounter() {
  frameCount.textContent = frameOf(video.currentTime) + ' / ' + frameOf(state.duration || 1);
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

// ---------- frame-stepping: moves the PLAYHEAD, not a marked point ----------
// Navigation is fully decoupled from marking now — ,/. and the «/‹/›/»
// buttons step the video's current position frame by frame, same as
// scrubbing or playback, not whichever point used to be "armed." Mark
// In/Out (below) capture wherever this navigation lands you, which is
// the actual point of the redesign: nudge to the exact frame first,
// then mark it, rather than nudging an already-marked point directly.
function step(deltaFrames) {
  var currentFrame = frameOf(video.currentTime);
  var next = clamp(currentFrame + deltaFrames, 0, frameOf(state.duration));
  video.currentTime = timeOfFrame(next);
  updateFrameCounter();
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

// Marking is now an explicit, deliberate capture — click Mark In (or
// press I) and whatever the playhead is doing RIGHT NOW becomes the new
// in-point, full stop. No more "whichever point happens to be armed
// silently follows every scrub/play/step you make" — that coupling was
// the actual source of the bar feeling finicky to use, not the dragging
// itself. Also arms that point for subsequent frame-stepping (,/.), so
// you can mark roughly then nudge precisely right after, in one flow.
selIn.onclick = function () { setHandleTime('in', video.currentTime, true); };
selOut.onclick = function () { setHandleTime('out', video.currentTime, true); };

// ---------- frame bar: keyboard ----------
// Same , / . convention as sakuga-enhancer.js, so the muscle memory carries
// over. Shift+,/. mirrors the medium-step buttons; the big ~1s jump is
// deliberately mouse/button-only (« / ») since it's a coarse repositioning
// move, not something worth a dedicated key. I/O mark In/Out at the
// current position — the standard convention in most editing tools,
// matching Mark In/Mark Out's button behavior exactly.
window.addEventListener('keydown', function (e) {
  // Don't hijack typing in the target-MB, filename, or other text fields.
  if (e.target.tagName === 'INPUT' && e.target.type !== 'range') return;

  if (e.key === ',') { step(e.shiftKey ? -MED_STEP : -1); e.preventDefault(); }
  else if (e.key === '.') { step(e.shiftKey ? MED_STEP : 1); e.preventDefault(); }
  else if (e.key === 'i' || e.key === 'I') { setHandleTime('in', video.currentTime, true); e.preventDefault(); }
  else if (e.key === 'o' || e.key === 'O') { setHandleTime('out', video.currentTime, true); e.preventDefault(); }
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
// ---------- preview audio (opt-in, muted by default) ----------
// Klippit is triggered from inside mpv/VLC, which is often still open
// with the same file — if this played audio by default and someone
// hadn't paused the original player, they'd get an echo. Muted by
// default (matching the <video> element's own attribute), one click to
// hear audio while editing if the original player is paused or closed.
var muteToggleBtn = document.getElementById('mute-toggle-btn');
var muteToggleX1 = document.getElementById('mute-toggle-x1');
var muteToggleX2 = document.getElementById('mute-toggle-x2');
var volumeSlider = document.getElementById('volume-slider');
muteToggleBtn.onclick = function () {
  video.muted = !video.muted;
  muteToggleBtn.setAttribute('aria-pressed', String(!video.muted));
  muteToggleBtn.title = video.muted
    ? 'Unmute preview audio (off by default — avoids echo if your original player is still open)'
    : 'Mute preview audio';
  // Plain speaker shape when unmuted, speaker-with-X when muted — kept
  // deliberately simple (just showing/hiding the X) rather than
  // swapping in animated sound-wave arcs for the unmuted state.
  muteToggleX1.style.display = video.muted ? '' : 'none';
  muteToggleX2.style.display = video.muted ? '' : 'none';
  // Only shown once unmuted — no point cluttering the seek-row with a
  // volume control in the default (muted) state.
  volumeSlider.style.display = video.muted ? 'none' : 'block';
};
volumeSlider.oninput = function () {
  video.volume = volumeSlider.value / 100;
};

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
    subtitleExternalFile: currentSubtitleExternalFile(),
    cropX: state.cropEnabled ? Math.round(state.cropX) : null,
    cropY: state.cropEnabled ? Math.round(state.cropY) : null,
    cropWidth: state.cropEnabled ? Math.round(state.cropWidth) : null,
    cropHeight: state.cropEnabled ? Math.round(state.cropHeight) : null
  }).then(function (path) {
    setStatus('screenshot saved — ' + path, 'done');
  }).catch(function (err) {
    setStatus('screenshot failed: ' + err, 'error');
  });
};

// ---------- trim handles: purely visual now (see render()) ----------
// Was previously draggable, both directly and as an alternative
// keyboard-nudge path. Removed entirely: it made the ruler feel like an
// interactive control competing with Mark In/Mark Out for the same job,
// and the "just a visual, nothing more" ask this came from is a genuine
// simplification, not a loss — the ruler still clearly shows where In
// and Out currently are, it just no longer does anything itself.

// ---------- seek bar (pure navigation — no side effects on In/Out) ----------
seekBar.addEventListener('input', function () {
  var pct = seekBar.value / 1000;
  video.currentTime = pct * state.duration;
});
video.addEventListener('timeupdate', function () {
  if (state.duration) seekBar.value = (video.currentTime / state.duration) * 1000;
  updateFrameCounter(); // live frame count during playback/scrubbing, not just discrete frame-steps
  // Playback and scrubbing are pure navigation — no side effects on
  // In/Out, and nothing here stops or redirects playback either. An
  // earlier "auto-pause at your Out marker" convenience got removed:
  // it fired any time playback naturally passed an *old* Out point,
  // including while just navigating toward a new mark, which
  // contradicted the whole point of making navigation fully free.
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
  // GIFs never carry audio at all, so the mute toggle is meaningless
  // there — hide it rather than leave a control with nothing to control.
  document.getElementById('audio-field').style.display = state.format === 'gif' ? 'none' : 'block';
  // GIF is palette-based, not H.264 — no GPU encoder involved at all.
  // gpu-field also sits inside panel-quality, so target-size mode
  // already hides it for free without needing a separate check here.
  document.getElementById('gpu-field').style.display = state.format === 'gif' ? 'none' : 'block';
  updateOutputExt();
});
bindSeg('mode-quality', 'mode-size', function (id) {
  state.mode = id === 'mode-quality' ? 'quality' : 'size';
  document.getElementById('panel-quality').style.display = state.mode === 'quality' ? 'block' : 'none';
  document.getElementById('panel-size').style.display = state.mode === 'size' ? 'block' : 'none';
});
bindSeg('audio-keep', 'audio-mute', function (id) {
  state.muteAudio = id === 'audio-mute';
});
bindSeg('gpu-off', 'gpu-on', function (id) {
  state.useGpu = id === 'gpu-on';
});

// ---------- crop ----------
// Static crop applied uniformly to the whole clip (the "actual video
// editor" territory — pan/follow-style animated cropping — was
// deliberately scoped out as a separate, much bigger feature). Crop
// coordinates live in SOURCE VIDEO PIXEL space throughout state, never
// screen pixels, converted to/from screen coordinates only at render
// and drag time via getVideoDisplayRect() below — this keeps the stored
// crop correct regardless of window size or preview letterboxing.
var cropOverlay = document.getElementById('crop-overlay');
var cropBox = document.getElementById('crop-box');
var cropMaskHole = document.getElementById('crop-mask-hole');
var cropDimensions = document.getElementById('crop-dimensions');
var cropSizeReadout = document.getElementById('crop-size-readout');
var ASPECT_RATIOS = { '16:9': 16 / 9, '9:16': 9 / 16, '1:1': 1 };

// Generic N-button segmented group — bindSeg() above only handles
// exactly two buttons, which the aspect-ratio row (four) doesn't fit.
function bindSegGroup(ids, onSelect) {
  var buttons = ids.map(function (id) { return document.getElementById(id); });
  buttons.forEach(function (btn, i) {
    btn.onclick = function () {
      buttons.forEach(function (b) { b.setAttribute('aria-pressed', 'false'); });
      btn.setAttribute('aria-pressed', 'true');
      onSelect(ids[i]);
    };
  });
}

// The actual rendered video rectangle within #preview-wrap, accounting
// for object-fit: contain's letterboxing/pillarboxing — the crop
// overlay has to be positioned against this, not #preview-wrap's own
// (generally differently-shaped) box, or the crop box would drift from
// the real video content whenever the source aspect ratio doesn't
// exactly match the preview area's shape.
//
// CROP_EDGE_MARGIN mirrors #preview-wrap.crop-active's CSS padding
// exactly (kept as one value here rather than reading computed styles,
// simpler and just as correct since this function is the only caller).
// Real usability fix: without it, a full-frame crop's corner handles
// sit right at the video's own edge, which can coincide with the actual
// OS window edge — easy to grab the window's own resize handle by
// mistake instead of a crop handle.
var CROP_EDGE_MARGIN = 10;
function getVideoDisplayRect() {
  var margin = state.cropEnabled ? CROP_EDGE_MARGIN : 0;
  var wrapW = previewWrap.clientWidth - margin * 2;
  var wrapH = previewWrap.clientHeight - margin * 2;
  var vw = state.sourceWidth || wrapW;
  var vh = state.sourceHeight || wrapH;
  if (!vw || !vh || !wrapW || !wrapH) return { left: margin, top: margin, width: wrapW, height: wrapH };
  var videoAspect = vw / vh;
  var wrapAspect = wrapW / wrapH;
  var dispW, dispH;
  if (videoAspect > wrapAspect) {
    dispW = wrapW;
    dispH = wrapW / videoAspect;
  } else {
    dispH = wrapH;
    dispW = wrapH * videoAspect;
  }
  return { left: margin + (wrapW - dispW) / 2, top: margin + (wrapH - dispH) / 2, width: dispW, height: dispH };
}

function renderCropOverlay() {
  previewWrap.classList.toggle('crop-active', state.cropEnabled);
  if (!state.cropEnabled || !state.sourceWidth || !state.sourceHeight || !state.cropWidth || !state.cropHeight) {
    cropOverlay.style.display = 'none';
    return;
  }
  cropOverlay.style.display = 'block';
  var disp = getVideoDisplayRect();
  var scaleX = disp.width / state.sourceWidth;
  var scaleY = disp.height / state.sourceHeight;

  var boxLeft = disp.left + state.cropX * scaleX;
  var boxTop = disp.top + state.cropY * scaleY;
  var boxWidth = state.cropWidth * scaleX;
  var boxHeight = state.cropHeight * scaleY;

  cropBox.style.left = boxLeft + 'px';
  cropBox.style.top = boxTop + 'px';
  cropBox.style.width = boxWidth + 'px';
  cropBox.style.height = boxHeight + 'px';

  cropMaskHole.setAttribute('x', boxLeft);
  cropMaskHole.setAttribute('y', boxTop);
  cropMaskHole.setAttribute('width', boxWidth);
  cropMaskHole.setAttribute('height', boxHeight);

  var label = Math.round(state.cropWidth) + ' \u00d7 ' + Math.round(state.cropHeight);
  cropDimensions.textContent = label;
  cropSizeReadout.textContent = label;
}
window.addEventListener('resize', renderCropOverlay);

// Back to the full source frame — the "undo" for crop isn't a history
// stack (see the design discussion this came out of): since crop is
// never destructive to the source, redefining the box or resetting it
// entirely covers everything a real undo would, without this being the
// one setting in the whole app with its own history tracking.
function resetCrop() {
  if (!state.sourceWidth || !state.sourceHeight) return;
  state.cropX = 0;
  state.cropY = 0;
  state.cropWidth = state.sourceWidth;
  state.cropHeight = state.sourceHeight;
  renderCropOverlay();
}

function applyCropAspect(ratioKey) {
  state.cropAspect = ratioKey;
  if (ratioKey === 'free' || !state.sourceWidth || !state.cropWidth) return;
  var ratio = ASPECT_RATIOS[ratioKey];
  var centerX = state.cropX + state.cropWidth / 2;
  var centerY = state.cropY + state.cropHeight / 2;
  // Fit to the current crop height first, falling back to width if that
  // would overflow the source frame — keeps the result as large as
  // reasonable while always staying fully inside the source.
  var newH = state.cropHeight;
  var newW = newH * ratio;
  if (newW > state.sourceWidth) { newW = state.sourceWidth; newH = newW / ratio; }
  if (newH > state.sourceHeight) { newH = state.sourceHeight; newW = newH * ratio; }
  state.cropWidth = newW;
  state.cropHeight = newH;
  state.cropX = clamp(centerX - newW / 2, 0, state.sourceWidth - newW);
  state.cropY = clamp(centerY - newH / 2, 0, state.sourceHeight - newH);
  renderCropOverlay();
}

document.getElementById('crop-reset-btn').onclick = function () {
  state.cropAspect = 'free';
  bindSegGroupReset();
  resetCrop();
};
function bindSegGroupReset() {
  ['crop-aspect-free', 'crop-aspect-169', 'crop-aspect-916', 'crop-aspect-11'].forEach(function (id) {
    document.getElementById(id).setAttribute('aria-pressed', id === 'crop-aspect-free' ? 'true' : 'false');
  });
}
bindSegGroup(['crop-aspect-free', 'crop-aspect-169', 'crop-aspect-916', 'crop-aspect-11'], function (id) {
  applyCropAspect({ 'crop-aspect-free': 'free', 'crop-aspect-169': '16:9', 'crop-aspect-916': '9:16', 'crop-aspect-11': '1:1' }[id]);
});

bindSeg('crop-off', 'crop-on', function (id) {
  state.cropEnabled = id === 'crop-on';
  if (state.cropEnabled && !state.cropWidth) resetCrop();
  document.getElementById('crop-controls').style.display = state.cropEnabled ? 'block' : 'none';
  renderCropOverlay();
});

// Drag inside the box (not on a handle) to move it without resizing.
cropBox.addEventListener('pointerdown', function (e) {
  if (e.target.classList.contains('crop-handle')) return; // handles have their own listener below
  var startX = e.clientX, startY = e.clientY;
  var startCropX = state.cropX, startCropY = state.cropY;
  var disp = getVideoDisplayRect();
  var scaleX = disp.width / state.sourceWidth;
  var scaleY = disp.height / state.sourceHeight;

  function onMove(ev) {
    var dxSource = (ev.clientX - startX) / scaleX;
    var dySource = (ev.clientY - startY) / scaleY;
    state.cropX = clamp(startCropX + dxSource, 0, state.sourceWidth - state.cropWidth);
    state.cropY = clamp(startCropY + dySource, 0, state.sourceHeight - state.cropHeight);
    renderCropOverlay();
  }
  function onUp() {
    document.removeEventListener('pointermove', onMove);
    document.removeEventListener('pointerup', onUp);
  }
  document.addEventListener('pointermove', onMove);
  document.addEventListener('pointerup', onUp);
});

// Corner handles resize from that corner, anchoring the opposite one.
// Edge handles resize along one axis only, anchoring the opposite edge
// — except when an aspect ratio is locked, where there's no natural
// single-edge anchor, so the box grows/shrinks symmetrically around its
// own center on the other axis instead.
['nw', 'ne', 'sw', 'se', 'n', 's', 'e', 'w'].forEach(function (handle) {
  document.getElementById('crop-handle-' + handle).addEventListener('pointerdown', function (e) {
    e.stopPropagation(); // don't also trigger the move-drag listener above
    var startX = e.clientX, startY = e.clientY;
    var startCrop = { x: state.cropX, y: state.cropY, w: state.cropWidth, h: state.cropHeight };
    var disp = getVideoDisplayRect();
    var scaleX = disp.width / state.sourceWidth;
    var scaleY = disp.height / state.sourceHeight;

    var hasW = handle.indexOf('w') !== -1;
    var hasE = handle.indexOf('e') !== -1;
    var hasN = handle.indexOf('n') !== -1;
    var hasS = handle.indexOf('s') !== -1;
    var hasHorizontal = hasW || hasE;
    var hasVertical = hasN || hasS;

    // Anchor per axis: the opposite edge for an axis this handle
    // actually drags, or the box's own center for an axis it doesn't
    // touch at all (only used if aspect ends up locked, since a locked
    // ratio still needs both dimensions to move together even from a
    // single-edge drag).
    var anchorX = hasW ? startCrop.x + startCrop.w : (hasE ? startCrop.x : startCrop.x + startCrop.w / 2);
    var anchorY = hasN ? startCrop.y + startCrop.h : (hasS ? startCrop.y : startCrop.y + startCrop.h / 2);

    function onMove(ev) {
      var dxSource = (ev.clientX - startX) / scaleX;
      var dySource = (ev.clientY - startY) / scaleY;

      var newW = startCrop.w, newH = startCrop.h;
      var newX = startCrop.x, newY = startCrop.y;

      if (hasHorizontal) {
        var draggedX = clamp((hasW ? startCrop.x : startCrop.x + startCrop.w) + dxSource, 0, state.sourceWidth);
        newW = Math.abs(draggedX - anchorX);
        newX = hasW ? anchorX - newW : anchorX;
      }
      if (hasVertical) {
        var draggedY = clamp((hasN ? startCrop.y : startCrop.y + startCrop.h) + dySource, 0, state.sourceHeight);
        newH = Math.abs(draggedY - anchorY);
        newY = hasN ? anchorY - newH : anchorY;
      }

      if (state.cropAspect !== 'free') {
        var ratio = ASPECT_RATIOS[state.cropAspect];
        if (hasHorizontal && !hasVertical) {
          // Pure width drag (e/w edge) — derive height from the ratio,
          // symmetric around the vertical center.
          newH = newW / ratio;
          newY = anchorY - newH / 2;
        } else if (hasVertical && !hasHorizontal) {
          // Pure height drag (n/s edge) — mirror of the above.
          newW = newH * ratio;
          newX = anchorX - newW / 2;
        } else {
          // Corner: horizontal movement drives the resize, same as a
          // free-aspect corner drag, height just follows the ratio.
          newH = newW / ratio;
          newY = hasN ? anchorY - newH : anchorY;
        }
      }

      state.cropWidth = newW;
      state.cropHeight = newH;
      // Re-clamp fully inside the source frame — aspect-locked resizing
      // (or a symmetric edge-drag expansion) can otherwise push an edge
      // past it.
      state.cropX = clamp(newX, 0, state.sourceWidth - newW);
      state.cropY = clamp(newY, 0, state.sourceHeight - newH);

      renderCropOverlay();
    }
    function onUp() {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
    }
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
  });
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
    // Edge case: crop was toggled on before metadata (and therefore
    // source dimensions) finished loading — initialize it now rather
    // than leaving the toggle showing "On" with no overlay ever
    // appearing, since nothing else would otherwise re-check this once
    // dimensions become known.
    if (state.cropEnabled && !state.cropWidth) { resetCrop(); }
    if (!meta.hasSubtitles) {
      disableSubtitleControls(noSubtitleMessage());
      disposeSubtitleOverlay();
    } else {
      // This ffprobe-based check is the reliable signal — re-enable
      // regardless of what mpv/VLC's active-track state guessed earlier
      // in applyInit(), since that only reflects whether a track was
      // actively selected in the player, not whether the file has one.
      // loadSubtitleOverlay() itself is NOT called here anymore — it's
      // already been kicked off speculatively in applyInit(), in
      // parallel with this metadata call, rather than waiting for this
      // confirmation first (see the comment there for why).
      subsOffBtn.disabled = false;
      subsOnBtn.disabled = false;
      subsStatus.textContent = '';
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
// Lazy-loads subtitles-octopus.js on first actual use rather than eagerly
// on every app startup via a <script> tag (removed from index.html) —
// this library was being parsed/executed on every single launch even
// when no file was loaded yet or the loaded file had no subtitles at
// all, which is most launches. Cached after the first load so switching
// between multiple subtitled files doesn't reload it repeatedly.
var subtitlesOctopusLoadPromise = null;
function ensureSubtitlesOctopusLoaded() {
  if (subtitlesOctopusLoadPromise) return subtitlesOctopusLoadPromise;
  subtitlesOctopusLoadPromise = new Promise(function (resolve, reject) {
    var script = document.createElement('script');
    script.src = 'lib/subtitles-octopus/subtitles-octopus.js';
    script.onload = resolve;
    script.onerror = function () { subtitlesOctopusLoadPromise = null; reject(new Error('failed to load subtitles-octopus.js')); };
    document.head.appendChild(script);
  });
  return subtitlesOctopusLoadPromise;
}

// Visible "loading" state on the CC button — the delay between a file
// with subtitles loading and text actually appearing is real and
// somewhat unavoidable given the current pipeline (extract .ass + fonts,
// fetch each as a blob, then load/initialize a ~2.7MB WASM library
// before anything can render), not a bug — confirmed real user
// confusion that it read as "subtitles just don't show up" rather than
// "still loading." This doesn't make it faster, just makes the wait
// visible instead of looking broken.
function setSubtitleLoadingIndicator(loading) {
  subsPreviewToggleBtn.classList.toggle('loading', loading);
  subsPreviewToggleBtn.title = loading
    ? 'Loading subtitles…'
    : 'Show/hide subtitle preview (also affects screenshots)';
}

function loadSubtitleOverlay() {
  disposeSubtitleOverlay();
  setSubtitleLoadingIndicator(true);
  ensureSubtitlesOctopusLoaded().then(function () {
    startSubtitleOverlayExtraction();
  }).catch(function (err) {
    setSubtitleLoadingIndicator(false);
    console.log('[klippit] failed to load subtitle overlay library:', err);
  });
}
function startSubtitleOverlayExtraction() {
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
          setSubtitleLoadingIndicator(false);
        },
        onError: function (err) {
          setSubtitleLoadingIndicator(false);
          console.log('[klippit] subtitle overlay error:', err);
        }
      });
      applySubsPreviewVisibility(); // harmless if canvas isn't ready yet — onReady above covers that case
    }).catch(function (err) {
      setSubtitleLoadingIndicator(false);
      console.log('[klippit] failed to prepare subtitle blob URLs:', err);
    });
  }).catch(function (err) {
    // Non-fatal — editing still works fine without the subtitle overlay,
    // this just means you won't see dialogue timing while trimming.
    setSubtitleLoadingIndicator(false);
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

var cancelBtn = document.getElementById('cancel-btn');

function setBusy(busy) {
  exportBtn.disabled = busy;
  spinnerEl.style.display = busy ? 'inline-block' : 'none';
  // While exporting, this button's job changes from "close the whole
  // panel" to "cancel the export in progress" — closing the panel
  // outright while ffmpeg is still running isn't something you'd want
  // anyway, so repurposing this slot fits naturally rather than adding
  // a second button just for the busy window.
  if (busy) {
    cancelBtn.textContent = 'Cancel';
    cancelBtn.onclick = cancelExport;
  } else {
    cancelBtn.textContent = 'Escape';
    cancelBtn.onclick = closePanel;
  }
}

function cancelExport() {
  if (!window.__TAURI__) return;
  cancelBtn.disabled = true;
  window.__TAURI__.core.invoke('cancel_export').catch(function (err) {
    console.log('[klippit] cancel_export failed:', err);
  }).then(function () {
    cancelBtn.disabled = false;
  });
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
    subtitleExternalFile: currentSubtitleExternalFile(),
    muteAudio: state.muteAudio,
    useGpu: state.useGpu,
    cropX: state.cropEnabled ? Math.round(state.cropX) : null,
    cropY: state.cropEnabled ? Math.round(state.cropY) : null,
    cropWidth: state.cropEnabled ? Math.round(state.cropWidth) : null,
    cropHeight: state.cropEnabled ? Math.round(state.cropHeight) : null
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

  window.__TAURI__.core.invoke('export_clip', { params: params }).then(function (result) {
    var path = result.outputPath;
    var encoderLabels = { h264_nvenc: 'NVIDIA GPU', h264_amf: 'AMD GPU', h264_qsv: 'Intel GPU' };
    var note = '';
    if (state.useGpu && result.encoderUsed === 'libx264') {
      // GPU was requested but every hardware option failed or wasn't
      // available — fell back to the regular CPU encoder automatically,
      // which is the whole point of asking for this to fail safely. Say
      // so plainly rather than silently using CPU without mentioning it.
      note = ' (GPU unavailable, used CPU instead)';
    } else if (encoderLabels[result.encoderUsed]) {
      note = ' (' + encoderLabels[result.encoderUsed] + ' encoding)';
    }
    setStatus('done' + note + ' — ' + path, 'done');
    lastExportedPath = path;
    statusActions.style.display = 'flex';
  }).catch(function (err) {
    // The Rust side returns the plain string "cancelled" specifically
    // for this case (see run_bin/cancel_export) — distinct from a
    // genuine ffmpeg failure, worth a clearer message than "export
    // failed: cancelled" would read as.
    if (err === 'cancelled') {
      setStatus('export cancelled', 'error');
    } else {
      setStatus('export failed: ' + err, 'error');
    }
  }).then(function () {
    setBusy(false);
  });
}
document.getElementById('export-btn').onclick = exportClip;
setBusy(false); // establishes the initial "Escape" label/behavior on cancel-btn

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
  filenameManuallyEdited = false;
  lastExportedPath = null;
  state.sourceWidth = 0;
  state.sourceHeight = 0;
  // Crop coordinates are tied to this specific file's dimensions —
  // reset per file, same as in/out. cropAspect (the "9:16" etc.
  // preference) deliberately isn't reset here, since that's a user
  // preference rather than file-specific data.
  state.cropEnabled = false;
  state.cropX = 0;
  state.cropY = 0;
  state.cropWidth = 0;
  state.cropHeight = 0;
  document.getElementById('crop-off').setAttribute('aria-pressed', 'true');
  document.getElementById('crop-on').setAttribute('aria-pressed', 'false');
  document.getElementById('crop-controls').style.display = 'none';
  previewWrap.classList.remove('crop-active');
  cropOverlay.style.display = 'none';
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

  var loadPrompt = document.getElementById('load-prompt');
  if (init.filePath) {
    if (loadPrompt) loadPrompt.style.display = 'none';
    video.src = window.__TAURI__
      ? window.__TAURI__.core.convertFileSrc(init.filePath)
      : 'file://' + init.filePath; // browser dev-preview fallback only
    video.load();
    // Kicked off speculatively here, in parallel with loadMetadata()
    // below, rather than waiting for its hasSubtitles confirmation
    // first — extraction and metadata are independent ffprobe/ffmpeg
    // calls on the same file, so there's no reason to run them one after
    // another. For a file with no subtitles this just fails harmlessly
    // (already handled gracefully), but for one that does have them,
    // this head start measurably reduces the real, somewhat unavoidable
    // delay before subtitle text actually appears — confirmed user
    // confusion that this delay read as "subtitles don't show up"
    // rather than "still loading."
    if (window.__TAURI__) loadSubtitleOverlay();
  } else if (loadPrompt) {
    // Opened directly with no file context (no mpv/VLC trigger) — offer
    // a way in rather than just sitting there empty. See load-browse-btn
    // and the drag-drop handling below.
    loadPrompt.style.display = 'flex';
  }

  loadMetadata();
  render();
}
// Exposed globally so main.rs's single-instance handler can call this
// directly on the already-open window via window.eval(...).
window.applyInit = applyInit;

// ---------- loading a file manually (Browse or drag-and-drop) ----------
// Only relevant when Klippit is opened without mpv/VLC context — see the
// load-prompt overlay in index.html, shown/hidden in applyInit() above
// based on whether a real file is currently loaded. Builds the same
// shape of object mpv/VLC's --init handoff produces and feeds it through
// that exact same applyInit() path, rather than duplicating any of its
// reset logic here.
function loadFileManually(filePath) {
  var fileName = filePath.split(/[\\/]/).pop();
  applyInit({
    filePath: filePath,
    fileName: fileName,
    startTime: 0,
    // Real subtitle detection happens via ffprobe in loadMetadata(),
    // same as every other trigger path — this initial guess is just the
    // same safe default VLC's own trigger always starts with.
    subtitle: { available: false }
  });
}

var loadBrowseBtn = document.getElementById('load-browse-btn');
if (loadBrowseBtn) {
  loadBrowseBtn.onclick = function () {
    if (!window.__TAURI__) {
      setStatus('file picker needs the Tauri backend (not available in dev preview)', 'error');
      return;
    }
    window.__TAURI__.dialog.open({
      multiple: false,
      filters: [{ name: 'Video', extensions: ['mp4', 'mkv', 'webm', 'avi', 'mov', 'm4v', 'flv', 'wmv', 'ts'] }]
    }).then(function (selected) {
      if (selected) loadFileManually(selected);
    }).catch(function (err) {
      setStatus('file picker failed: ' + err, 'error');
    });
  };
}

// UNVERIFIED: Tauri v2's dedicated drag-drop event API is new surface
// area not exercised elsewhere in this project — the exact namespace/
// method/payload shape here is my best understanding, not confirmed
// against a real build. If dropping a file doesn't work, the Browse
// button above is a fully reliable fallback regardless; this is a
// bonus convenience layered on top, not the only way in.
//
// Deferred via setTimeout rather than run immediately at script load —
// this isn't needed for the initial render at all (nothing visible
// depends on it), so there's no reason for it to compete with getting
// pixels on screen first. The delay is imperceptible to a person but
// keeps this off the critical startup path.
setTimeout(function setUpDragDrop() {
  if (!window.__TAURI__ || !window.__TAURI__.window) return;
  try {
    var win = window.__TAURI__.window.getCurrentWebviewWindow();
    win.onDragDropEvent(function (event) {
      var payload = event.payload || {};
      var loadPrompt = document.getElementById('load-prompt');
      if (payload.type === 'over') {
        if (loadPrompt) loadPrompt.classList.add('dragover');
      } else if (payload.type === 'drop' && payload.paths && payload.paths.length) {
        if (loadPrompt) loadPrompt.classList.remove('dragover');
        loadFileManually(payload.paths[0]);
      } else {
        if (loadPrompt) loadPrompt.classList.remove('dragover');
      }
    });
  } catch (err) {
    console.log('[klippit] drag-drop event API unavailable:', err);
  }
}, 0);


video.addEventListener('loadedmetadata', function () {
  state.duration = video.duration || 0;
  video.currentTime = state.inTime;
  render();
  renderCropOverlay();
});

// Called from main.rs's background setup check (env var + mpv/VLC script
// install) via window.eval — only if something actually failed. Shows a
// dismissible banner rather than a silent failure; a full log always
// lives at %TEMP%\klippit-setup.log regardless of whether this fires.
window.__klippitSetupWarning = function (message) {
  var banner = document.getElementById('setup-warning');
  var text = document.getElementById('setup-warning-text');
  if (!banner || !text) return;
  text.textContent = 'Setup check: ' + message;
  banner.style.display = 'flex';
};
var setupWarningDismiss = document.getElementById('setup-warning-dismiss');
if (setupWarningDismiss) {
  setupWarningDismiss.onclick = function () {
    document.getElementById('setup-warning').style.display = 'none';
  };
}

// ---------- settings panel ----------
// Klippit's first settings surface — lets the mpv keybind and mpv config
// folder (for portable_config setups) be changed from inside the app,
// rather than requiring anyone to edit clip-trigger.lua or input.conf by
// hand. Real feedback drove both fields: someone's mpv used
// portable_config so the script landed somewhere mpv never looked, and
// a separate person pointed out that requiring a script edit just to
// change a keybind defeats the point of the app being easier than
// hand-rolled ffmpeg.
var settingsModal = document.getElementById('settings-modal');
var settingsBtn = document.getElementById('settings-btn');
var settingsStatus = document.getElementById('settings-status');
var settingsKeybindInput = document.getElementById('settings-mpv-keybind');
var settingsMpvDirInput = document.getElementById('settings-mpv-dir');

function openSettings() {
  settingsStatus.textContent = '';
  if (window.__TAURI__) {
    window.__TAURI__.core.invoke('get_settings').then(function (settings) {
      settingsKeybindInput.value = settings.mpvKeybind || 'c';
      settingsMpvDirInput.value = settings.mpvConfigDirOverride || '';
    }).catch(function (err) {
      settingsStatus.textContent = 'Could not load current settings: ' + err;
    });
  }
  settingsModal.style.display = 'flex';
}
function closeSettings() {
  settingsModal.style.display = 'none';
}
if (settingsBtn) settingsBtn.onclick = openSettings;
document.getElementById('settings-close-btn').onclick = closeSettings;
document.getElementById('settings-save-btn').onclick = function () {
  if (!window.__TAURI__) {
    settingsStatus.textContent = 'Settings need the Tauri backend (not available in dev preview).';
    return;
  }
  settingsStatus.textContent = 'Saving and reinstalling…';
  window.__TAURI__.core.invoke('save_settings_and_reinstall', {
    mpvKeybind: settingsKeybindInput.value,
    mpvConfigDirOverride: settingsMpvDirInput.value || null
  }).then(function (report) {
    settingsStatus.textContent = report;
  }).catch(function (err) {
    settingsStatus.textContent = 'Failed: ' + err;
  });
};
// Click-outside-to-close, same convention as most modal dialogs.
settingsModal.addEventListener('mousedown', function (e) {
  if (e.target === settingsModal) closeSettings();
});

updateOutputExt();
applyInit(init);
