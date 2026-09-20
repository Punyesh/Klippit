--[[
Klippit — VLC extension (menu-triggered)

Triggered via VLC's View > Extensions > "Send to Klippit" menu item —
keyboard-reachable without a mouse via Alt, then V (View), arrow down to
Extensions, Enter on this item. VLC extensions can't be bound to a
single dedicated hotkey the way mpv scripts (or even VLC's own
interface-script key-pressed hook) can — this is the tradeoff for using
VLC's more standard, better-documented scripting surface instead of the
speculative key-pressed technique that didn't pan out.

INSTALL: copy this file to %APPDATA%\vlc\lua\extensions\klippit.lua
(create the `extensions` folder if it doesn't exist — sibling to `intf`,
not inside it). No Preferences changes needed at all — VLC auto-discovers
scripts in this folder and lists them under View > Extensions on next
launch.

This does its work immediately on activation with no dialog or visible
window of its own — from your perspective, clicking the menu item *is*
the trigger, same as pressing mpv's key.
]]

local function get_klippit_path()
    return os.getenv("KLIPPIT_PATH")
end

local function url_decode(s)
    s = s:gsub("+", " ")
    s = s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end)
    return s
end

local function uri_to_path(uri)
    local path = uri:gsub("^file:///", "")
    return url_decode(path)
end

local function json_escape(s)
    return (s:gsub('\\', '\\\\'):gsub('"', '\\"'))
end

function descriptor()
    return {
        title = "Send to Klippit",
        version = "1.0",
        shortdesc = "Klippit",
        description = "Sends the current file and timestamp to Klippit for clipping.",
        capabilities = {}
    }
end

-- Called the moment the menu item is clicked — no dialog, does its job
-- and returns immediately.
function activate()
    local item = vlc.input.item()
    if not item then
        vlc.msg.warn("[klippit] no file playing")
        return
    end

    local path = uri_to_path(item:uri())
    local filename = path:match("([^/\\]+)$") or path

    local input = vlc.object.input()
    -- VLC's "time" variable is in microseconds.
    local time_us = input and vlc.var.get(input, "time") or 0
    local start_time = time_us / 1000000

    local app_path = get_klippit_path()
    if not app_path then
        vlc.msg.err("[klippit] KLIPPIT_PATH environment variable not set")
        return
    end

    -- Subtitle-track detection isn't implemented for the VLC path yet —
    -- see the same note in vlc-scripts/klippit-intf.lua.
    local json = string.format(
        '{"filePath":"%s","fileName":"%s","startTime":%.3f,"subtitle":{"available":false}}',
        json_escape(path), json_escape(filename), start_time
    )

    local escaped_json = json:gsub('"', '\\"')
    local cmd = string.format('start "" "%s" --init "%s"', app_path, escaped_json)
    os.execute(cmd)

    vlc.msg.info("[klippit] opening at " .. string.format("%.2fs", start_time))
end

function deactivate()
end

function close()
end
