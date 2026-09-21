; Klippit — post-install setup hook
;
; UNVERIFIED: the macro name (NSIS_HOOK_POSTINSTALL) and the
; tauri.conf.json key that wires this file in ("bundle.windows.nsis.
; installerHooks") are my best understanding of Tauri v2's NSIS
; customization mechanism, not confirmed against a real build — there's
; no Windows/NSIS toolchain available to test this in. If
; `cargo tauri build` fails specifically because of this file or that
; config key, that's the exact thing to paste back — the fix is likely
; just the correct macro/key name, not a rethink of the approach.
;
; Nothing else in this setup mechanism depends on this hook actually
; firing. The app's own background self-healing check (see run_setup()
; in main.rs) configures the same things — KLIPPIT_PATH, the mpv script,
; the VLC extension — every time Klippit itself launches normally,
; regardless of whether this hook worked. This file only controls
; whether setup happens literally during the install wizard instead of
; a moment after the first real launch — a smoother experience if it
; works, not a required one.
;
; $INSTDIR is NSIS's built-in variable for the install directory Tauri
; is already using elsewhere in its generated script.

!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\klippit.exe" --setup'
!macroend
