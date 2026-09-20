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

This does its work immediately on activation — no dialog appears on
success, clicking the menu item *is* the trigger, same as pressing
mpv's key. A dialog only appears if something goes wrong (no file
detected, KLIPPIT_PATH unset, or any other error), so you have something
concrete to report rather than a silent failure.
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

-- Called the moment the menu item is clicked. Wrapped in pcall so that
-- whatever is actually going wrong becomes visible as a dialog, rather
-- than vanishing silently the way a couple of earlier theories about
-- this (a nil vlc.input.item(), a missing KLIPPIT_PATH) turned out to —
-- clicking the menu item produced literally no feedback at all in
-- testing, meaning something was failing before even reaching those
-- checks, most likely a plain Lua error VLC was swallowing.
function activate()
    local ok, err = pcall(function()
        local item = vlc.input.item()
        if not item then
            local d = vlc.dialog("Klippit")
            d:add_label("No active file detected yet — wait a moment and try again.", 1, 1, 2, 1)
            d:show()
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
            local d = vlc.dialog("Klippit")
            d:add_label("KLIPPIT_PATH environment variable is not set.", 1, 1, 2, 1)
            d:show()
            vlc.msg.err("[klippit] KLIPPIT_PATH environment variable not set")
            return
        end

        -- Best-effort subtitle-language detection: VLC's "spu-es"
        -- variable holds the current subtitle track's numeric ID; its
        -- description text (from get_list) is usually a human-readable
        -- name like "English" rather than a clean ISO code like mpv
        -- reports — the Rust side's matching is deliberately lenient
        -- (substring match either direction) to give this a real chance
        -- against ffprobe's "eng"-style tags, but this is genuinely the
        -- least-tested part of the whole subtitle pipeline: VLC's exact
        -- Lua API surface for this hasn't gone well on the first few
        -- guesses in this project, so if language matching doesn't
        -- work via VLC, it just falls back to "first stream found" —
        -- same as before this feature existed, not worse.
        --
        -- External subtitle files (VLC's "Add Subtitle File") are NOT
        -- detected here — an earlier attempt guessed a full path from
        -- just a filename (VLC's API doesn't expose the real path
        -- directly), which was too speculative to ship. "trigger":"vlc"
        -- below lets the app show an honest "not supported" message
        -- instead of silently guessing wrong.
        local sub_lang = nil
        local ok_lang, spu_id = pcall(vlc.var.get, input, "spu-es")
        if ok_lang and spu_id and spu_id ~= -1 then
            local ok_list, values, texts = pcall(vlc.var.get_list, input, "spu-es")
            if ok_list and values and texts then
                for i, v in ipairs(values) do
                    if v == spu_id then
                        sub_lang = texts[i]
                        break
                    end
                end
            end
        end

        local json = string.format(
            '{"filePath":"%s","fileName":"%s","startTime":%.3f,"trigger":"vlc","subtitle":{"available":%s,"lang":%s}}',
            json_escape(path), json_escape(filename), start_time,
            sub_lang and "true" or "false",
            sub_lang and ('"' .. json_escape(sub_lang) .. '"') or "null"
        )

        local escaped_json = json:gsub('"', '\\"')
        local cmd = string.format('start "" "%s" --init "%s"', app_path, escaped_json)
        os.execute(cmd)

        vlc.msg.info("[klippit] opening at " .. string.format("%.2fs", start_time))
    end)

    if not ok then
        -- This is the important part: whatever actually broke is now
        -- visible on screen, not just in a Messages window nobody has
        -- open. Please paste back exactly what this dialog says.
        local d = vlc.dialog("Klippit — Error")
        d:add_label(tostring(err), 1, 1, 2, 1)
        d:show()
        vlc.msg.err("[klippit] activate() error: " .. tostring(err))
    end
end

function deactivate()
end

function close()
end
