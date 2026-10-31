--!nonstrict
-- SPDX-License-Identifier: GPL-3.0-or-later
-- One subprocess at a time; intermediate queries are replaced by the latest.
-- This adapter only reads the index. It never starts/reconfigures an indexer.
local generation = 0
local busy = false
local pending = nil
local paths = {}
local search

local function executable()
    local binary = noctalia.getConfig("binary")
    return binary:sub(1, 1) == "~" and noctalia.expandPath(binary) or binary
end

local function message(query, key)
    launcher.setResults(query, {{ id = "message", title = noctalia.tr(key), glyph = "info-circle" }})
end

local function publish(query, result)
    if result.timedOut or result.stdoutTruncated or (result.exitCode ~= 0 and result.exitCode ~= 1) then
        message(query, "search_failed")
        return
    end
    local ok, response = pcall(noctalia.json.decode, result.stdout)
    if not ok or type(response) ~= "table" or response.version ~= 1 or type(response.results) ~= "table" then
        message(query, "invalid_response")
        return
    end
    local rows = {}
    for _, hit in ipairs(response.results) do
        -- A non-UTF-8 Unix path has path=null and path_bytes in the CLI.
        -- Never open its lossy display name or guess a replacement path.
        if type(hit) == "table" and type(hit.path) == "string" and hit.path:sub(1, 1) == "/"
            and not hit.path:find("\0", 1, true) then
            local id = "file:" .. tostring(generation) .. ":" .. tostring(#rows + 1)
            paths[id] = hit.path
            rows[#rows + 1] = {
                id = id,
                title = type(hit.name) == "string" and hit.name or hit.path,
                subtitle = hit.path,
                glyph = hit.kind == "directory" and "folder" or "file",
                -- Retain FerroSeek's final order, including tie-breaking.
                score = 1000 - #rows,
            }
        end
    end
    if #rows == 0 then
        message(query, #response.results > 0 and "unsupported_paths" or "no_results")
    else
        launcher.setResults(query, rows)
    end
end

search = function(request)
    busy = true
    local binary = executable()
    local argv = { binary, "search", "--json", "--no-hidden", "-n", tostring(noctalia.getConfig("max_results")) }
    local directory = noctalia.getConfig("directory")
    if directory ~= "" then
        argv[#argv + 1] = "--in"
        argv[#argv + 1] = noctalia.expandPath(directory)
    end
    argv[#argv + 1] = "--"
    argv[#argv + 1] = request.query
    local accepted = noctalia.runAsync(argv, function(result)
        busy = false
        if request.generation == generation then
            publish(request.query, result)
        end
        local nextRequest = pending
        pending = nil
        if nextRequest ~= nil then
            search(nextRequest)
        end
    end)
    if not accepted then
        busy = false
        if request.generation == generation then
            message(request.query, "search_failed")
        end
    end
end

function onQuery(text)
    generation = generation + 1
    pending = nil
    paths = {}
    launcher.setResults(text, {})
    if text:match("^%s*$") then
        message(text, "hint")
        return
    end
    local binary = executable()
    if not noctalia.commandExists(binary) then
        message(text, "missing_binary")
        return
    end
    message(text, "searching")
    local request = { query = text, generation = generation }
    if busy then
        pending = request
    else
        search(request)
    end
end

function onActivate(id)
    local path = paths[id]
    if path == nil then
        return
    end
    if not noctalia.fileExists(path) then
        noctalia.notifyError("FerroSeek", noctalia.tr("file_missing"))
        return
    end
    if not noctalia.runAsync({ "xdg-open", path }) then
        noctalia.notifyError("FerroSeek", noctalia.tr("open_failed"))
    end
end
