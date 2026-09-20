-- Klippit — mpv trigger
--
-- Bound to a key (default: c). Grabs the current file path, timestamp, and
-- active subtitle info from mpv, then launches (or messages, if already
-- running) the Tauri clip panel with that context. This script does no
-- video processing itself — it's pure handoff, matching the split we
-- settled on: mpv owns playback + capture, the panel owns the UI + export.

local utils = require 'mp.utils'

-- TODO: point this at wherever the built app actually lands for your OS —
-- this scaffold hasn't been built/packaged, so there's no real binary yet.
local APP_PATH = "/path/to/klippit"

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

    -- TODO: this always spawns a new instance. A real single-instance setup
    -- should check for an existing app window (e.g. via a local socket the
    -- Tauri app listens on at startup) and send it a "load this clip"
    -- message instead of launching a duplicate — worth doing before this
    -- becomes daily-driver software, since re-opening a panel over and
    -- over mid-episode is exactly the friction this tool is meant to avoid.
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
