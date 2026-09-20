-- Klippit — mpv trigger
--
-- Bound to a key (default: c). Grabs the current file path, timestamp, and
-- active subtitle info from mpv, then launches (or messages, if already
-- running) the Tauri clip panel with that context. This script does no
-- video processing itself — it's pure handoff, matching the split we
-- settled on: mpv owns playback + capture, the panel owns the UI + export.

local utils = require 'mp.utils'

-- Reads the app's location from a KLIPPIT_PATH environment variable if
-- set, falling back to the literal path below otherwise. This means
-- switching from a dev build to an installed release build (or moving
-- the app at all) only ever needs a Windows environment variable
-- updated once, never an edit to this file — see the README for how to
-- set KLIPPIT_PATH.
local APP_PATH = os.getenv("KLIPPIT_PATH") or "/path/to/klippit"

local function get_subtitle_context()
    local sid = mp.get_property("sid") -- "no" if none selected
    if sid == "no" or sid == nil then
        return { available = false }
    end
    return {
        available = true,
        track_id = sid,
        external_file = mp.get_property("current-tracks/sub/external-filename"),
        delay = mp.get_property_number("sub-delay") or 0
    }
end

local function open_clip_tool()
    if APP_PATH == "/path/to/klippit" then
        mp.osd_message("klippit: KLIPPIT_PATH environment variable not set — see README", 5)
        return
    end

    local path = mp.get_property("path")
    if not path then
        mp.osd_message("klippit: no file playing")
        return
    end

    local payload = {
        filePath = utils.join_path(mp.get_property("working-directory"), path),
        fileName = mp.get_property("filename"),
        startTime = mp.get_property_number("time-pos") or 0,
        subtitle = get_subtitle_context()
    }

    -- The app itself now handles single-instance behavior (see main.rs's
    -- tauri-plugin-single-instance setup): if a Klippit window is already
    -- open, this spawned process hands its --init argv to that existing
    -- window and exits immediately, rather than a second window opening.
    -- This script's job stays the same either way — just capture context
    -- and launch/hand-off.
    local json = utils.format_json(payload)
    local args = { APP_PATH, "--init", json }

    mp.command_native_async(
        { name = "subprocess", args = args, playback_only = false, detach = true },
        function (success, result, error)
            if not success then
                mp.osd_message("klippit: failed to launch (" .. tostring(error) .. ")")
            end
        end
    )

    mp.osd_message("klippit: opening at " .. string.format("%.2fs", payload.startTime))
end

mp.add_key_binding("c", "open-klippit", open_clip_tool)
