--[[
Klippit — VLC trigger (interface script)

STATUS: the most speculative piece of this whole project. mpv has a
clean, documented API (mp.add_key_binding) built exactly for "run this
function when this key is pressed." VLC does not have a direct
equivalent for interface scripts — this instead observes VLC's internal
"key-pressed" libvlc variable, which fires on every keypress VLC sees,
and checks whether it matches the key we care about. This pattern shows
up in a handful of community VLC scripts but is thin on official
documentation, so treat this as "should work" rather than "confirmed
working" until you've actually tested it.

INSTALL:
  1. Copy this file to: %APPDATA%\vlc\lua\intf\klippit.lua
  2. In VLC: Tools > Preferences > Show settings: All (bottom left) >
     Interface > Main interfaces > check "Lua"
  3. Still under Interface > Main interfaces > Lua: set "Lua interface"
     to "klippit" (no .lua extension)
  4. Save, fully quit and restart VLC (not just close the window —
     interface changes need a real restart)
  5. Set KLIPPIT_PATH the same way as for mpv (see main README) — this
     script reads the same environment variable.

DEBUGGING: VLC's Tools > Messages window (set verbosity to at least 1)
is where vlc.msg.info/warn/err output below actually shows up — nothing
here is visible anywhere else. If pressing 'c' does nothing at all, open
that window and check whether ANY [klippit] message appears when VLC
starts (confirms the script loaded) and when you press the key
(confirms the callback fired). If the script doesn't even load, the
--intf/Lua interface setting above is the most likely culprit; if it
loads but pressing 'c' produces nothing, the key-pressed matching logic
below is the most likely culprit — see the comment on KEY_C.
]]

local function get_klippit_path()
    return os.getenv("KLIPPIT_PATH")
end

local function url_decode(s)
    s = s:gsub("+", " ")
    s = s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end)
    return s
end

-- VLC gives file paths as a URI, e.g. file:///C:/Users/you/video.mp4 on
-- Windows. Strip the scheme and decode percent-escapes (spaces as %20,
-- etc.) to get back a real filesystem path.
local function uri_to_path(uri)
    local path = uri:gsub("^file:///", "")
    return url_decode(path)
end

local function json_escape(s)
    return (s:gsub('\\', '\\\\'):gsub('"', '\\"'))
end

local function trigger_klippit()
    local item = vlc.input.item()
    if not item then
        vlc.msg.warn("[klippit] no file playing")
        return
    end

    local path = uri_to_path(item:uri())
    local filename = path:match("([^/\\]+)$") or path

    local input = vlc.object.input()
    -- VLC's "time" variable is in microseconds; mpv's time-pos is
    -- already in seconds, hence the /1000000 here that mpv's script
    -- doesn't need.
    local time_us = input and vlc.var.get(input, "time") or 0
    local start_time = time_us / 1000000

    local app_path = get_klippit_path()
    if not app_path then
        vlc.msg.err("[klippit] KLIPPIT_PATH environment variable not set")
        return
    end

    -- Subtitle-track detection isn't implemented for the VLC path yet —
    -- pulling the currently selected subtitle track reliably out of
    -- VLC's Lua API needs more investigation than this first pass
    -- covers, so this always reports "no subtitles" for now. The mpv
    -- path (clip-trigger.lua) does this properly.
    local json = string.format(
        '{"filePath":"%s","fileName":"%s","startTime":%.3f,"trigger":"vlc","subtitle":{"available":false}}',
        json_escape(path), json_escape(filename), start_time
    )

    -- Interface scripts (unlike the more sandboxed "extension" scripts)
    -- have os.execute available, so we can shell out directly rather
    -- than needing VLC's own process-spawning API.
    local escaped_json = json:gsub('"', '\\"')
    local cmd = string.format('start "" "%s" --init "%s"', app_path, escaped_json)
    os.execute(cmd)

    vlc.msg.info("[klippit] opening at " .. string.format("%.2fs", start_time))
end

-- The key we're watching for. VLC's key-pressed variable delivers a
-- numeric code, not a named constant the way mpv exposes key names —
-- this assumes it's the plain ASCII byte value for lowercase 'c' with no
-- modifier bits set, which may not match VLC's actual encoding. If this
-- doesn't fire, temporarily add `vlc.msg.info("[klippit] key: " .. new)`
-- as the first line of key_pressed below, watch the Messages window
-- while pressing various keys, and use whatever value actually shows up
-- for 'c' instead of string.byte("c").
local KEY_C = string.byte("c")

local function key_pressed(var, old, new, data)
    if new == KEY_C then
        trigger_klippit()
    end
end

vlc.var.add_callback(vlc.object.libvlc(), "key-pressed", key_pressed)
vlc.msg.info("[klippit] interface script loaded, watching for 'c'")

-- Interface scripts need to keep running (VLC unloads a script that
-- returns immediately) — this just idles until VLC itself shuts down.
while not vlc.misc.should_die() do
    vlc.misc.mwait(vlc.misc.mdate() + 500000)
end

vlc.var.del_callback(vlc.object.libvlc(), "key-pressed", key_pressed)
