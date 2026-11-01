-- SPDX-License-Identifier: GPL-3.0-or-later
-- The provider uses the Lua/Luau common subset. Exercise its real callbacks
-- with a mocked Noctalia host; never connect to the desktop or open a file.
local script = (arg[1] or "integrations/noctalia/ferroseek") .. "/launcher.luau"
local calls, published, notices, config, available, exists, accept, decoded

local function reset()
    calls, published, notices = {}, {}, {}
    config = { binary = "ferroseek", directory = "", max_results = 30 }
    available, exists, accept = true, true, true
    decoded = { version = 1, results = {} }
    noctalia = {
        getConfig = function(key) assert(config[key] ~= nil); return config[key] end,
        expandPath = function(path) return (path:gsub("^~", "/test/home")) end,
        commandExists = function() return available end,
        tr = function(key) return key end,
        json = { decode = function(text)
            assert(text ~= "malformed", "invalid JSON")
            return decoded
        end },
        runAsync = function(argv, callback)
            assert(type(argv) == "table", "dynamic commands must use argv")
            calls[#calls + 1] = { argv = argv, callback = callback }
            return accept
        end,
        fileExists = function() return exists end,
        notifyError = function(_, text) notices[#notices + 1] = text end,
    }
    launcher = { setResults = function(query, rows)
        published[#published + 1] = { query = query, rows = rows }
    end }
    assert(loadfile(script))()
end

local function latest() return published[#published] end
local function title() return latest().rows[1].title end
local function finish(index, overrides)
    local result = { exitCode = 0, stdout = "json", stderr = "" }
    for k, v in pairs(overrides or {}) do result[k] = v end
    assert(calls[index].callback)(result)
end
local function hit(path, name, kind)
    return { path = path, name = name or "report.txt", kind = kind or "file", score = 42 }
end

reset()
onQuery("   ")
assert(#calls == 0 and title() == "hint")
onActivate("message")
assert(#calls == 0)
available = false
onQuery("report")
assert(#calls == 0 and title() == "missing_binary")

reset()
config.binary, config.directory = "~/bin/Ferro Seek", "~/My documents"
local query = "--help $(touch /tmp/noctalia-must-not-run); 'quoted'"
onQuery(query)
local argv = calls[1].argv
assert(table.concat(argv, "|") == "/test/home/bin/Ferro Seek|search|--json|--no-hidden|-n|30|--in|/test/home/My documents|--|" .. query)
local path = "/test/report \"caffè\"\n$(touch nope).txt"
decoded.results = { hit(path, "report caffè"), hit("/test/folder", "folder", "directory") }
finish(1)
assert(latest().query == query and #latest().rows == 2)
assert(title() == "report caffè" and latest().rows[2].glyph == "folder")
assert(latest().rows[1].score > latest().rows[2].score)
onActivate("made-up-id")
assert(#calls == 1)
onActivate(latest().rows[1].id)
assert(#calls == 2 and calls[2].argv[1] == "xdg-open" and calls[2].argv[2] == path and #calls[2].argv == 2)
finish(2)
assert(#notices == 0, "successful opening must not report an error")

for _, failure in ipairs({ {exitCode = 4}, {exitCode = 0, timedOut = true} }) do
    reset()
    onQuery("report")
    decoded.results = { hit("/test/report.txt") }
    finish(1)
    onActivate(latest().rows[1].id)
    assert(#notices == 0, "opening failure is only known when the process completes")
    -- Completion may arrive after the launcher moved on to another query.
    onQuery("")
    finish(2, failure)
    assert(#notices == 1 and notices[1] == "open_failed", "report asynchronous opener failures")
end

reset()
onQuery("a")
onQuery("b")
onQuery("c")
assert(#calls == 1, "at most one search may be in flight")
decoded.results = { hit("/stale") }
finish(1)
assert(#calls == 2 and calls[2].argv[#calls[2].argv] == "c")
assert(title() == "searching", "stale result must not be published")
decoded.results = { hit("/current") }
finish(2)
assert(latest().query == "c" and latest().rows[1].subtitle == "/current")
local oldId = latest().rows[1].id
onQuery("d")
decoded.results = { hit("/different") }
finish(3)
onActivate(oldId)
assert(#calls == 3, "an old result id must not select a new query's row")

reset()
onQuery("a")
onQuery("b")
onQuery("")
finish(1)
assert(#calls == 1 and title() == "hint", "clearing the query drops queued work")

reset()
onQuery("a")
onQuery("b")
onQuery("a")
decoded.results = { hit("/old-a") }
finish(1)
assert(#calls == 2 and title() == "searching", "same text must not resurrect an old generation")
decoded.results = { hit("/new-a") }
finish(2)
assert(latest().rows[1].subtitle == "/new-a")

for _, failure in ipairs({ {exitCode = 2}, {timedOut = true}, {stdoutTruncated = true} }) do
    reset()
    onQuery("report")
    finish(1, failure)
    assert(title() == "search_failed")
    onActivate("message")
    assert(#calls == 1)
end
reset()
accept = false
onQuery("report")
assert(title() == "search_failed")
accept = true
onQuery("again")
assert(#calls == 2, "a failed launch must release the busy flag")

reset()
onQuery("report")
finish(1, {exitCode = 1})
assert(title() == "no_results", "exit 1 means no matches, not an index error")
reset()
onQuery("report")
finish(1, {stdout = "malformed"})
assert(title() == "invalid_response")
reset()
onQuery("report")
decoded.version = 2
finish(1)
assert(title() == "invalid_response")

reset()
onQuery("report")
decoded.results = { hit(false), hit("relative.txt"), hit("/bad\0path"), hit("/safe") }
finish(1)
assert(#latest().rows == 1 and latest().rows[1].subtitle == "/safe")
local id = latest().rows[1].id
exists = false
onActivate(id)
assert(#calls == 1 and notices[1] == "file_missing")
exists, accept = true, false
onActivate(id)
assert(notices[2] == "open_failed")
onQuery("new")
onActivate(id)
assert(#calls == 3, "results from the previous query must not remain activatable")

reset()
onQuery("report")
decoded.results = { hit(false) }
finish(1)
assert(title() == "unsupported_paths")

print("Noctalia provider: all callback tests passed")
