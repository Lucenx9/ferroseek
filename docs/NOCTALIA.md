# Noctalia launcher integration

The optional `lucenx9/ferroseek` plugin adds filename/path search to Noctalia
v5's existing launcher. Type `/ferro report`, select a result with the arrow
keys and press Enter to open it with the default application. Directories
open in the file manager. No KDE, KRunner or MCP server is needed.

## Requirements and installation

- Noctalia v5 with plugin API **24 or newer** (argument-array subprocesses).
  Noctalia v4/QML plugins use a different API and are not supported.
- The FerroSeek CLI from the same checkout, with `search --json` support.
- `xdg-open` from `xdg-utils` and a configured default file manager/applications.

From the FerroSeek checkout, install the core if needed:

```sh
./scripts/install.sh --core
```

Review `ferroseek config` and choose the indexing roots/exclusions in the
reported configuration file **before** creating an index. The engine's default
root is the home directory. Then run `ferroseek index`, or explicitly start
the installed user service for live updates:

```sh
systemctl --user enable --now ferroseek.service
```

Searching also works with a stopped daemon; the index then remains as fresh
as its last indexing run. The plugin never builds/rebuilds the index, starts
a daemon, expands indexing roots or changes other desktop integrations.

Add the optional plugin source and enable it:

```sh
# Run from the FerroSeek checkout; keep this checkout at this location.
noctalia msg plugins source add ferroseek path "$PWD/integrations/noctalia"
noctalia msg plugins enable lucenx9/ferroseek
```

Open the launcher with your existing keybinding (for example Super+Space on
Niri) and type `/ferro report`. `/` also lists the provider alongside the
built-in providers. Plugin changes are not installed automatically by the
FerroSeek installer; this source-checkout adapter is not part of the native
component archives or Noctalia's community catalog.

In **Noctalia Settings → Plugins → FerroSeek**, configure:

| Setting | Default | Meaning |
|---|---|---|
| FerroSeek executable | `ferroseek` | Executable name on Noctalia's PATH, or an absolute/`~/` path such as `~/.local/bin/ferroseek` |
| Search directory | empty | Optional `--in` scope inside the existing index; it does not authorize or index a new root |
| Maximum results | 30 | Between 1 and 100 |

The provider only participates behind `/ferro`. It does not add file results
to ordinary app searches. Hidden paths are always excluded by `--no-hidden`.
This is a local desktop adapter using the CLI: **MCP allowlists and sensitive
name filters do not apply**. Other non-hidden indexed paths are searchable,
subject to the optional search-directory scope.

To disable/remove the source without touching the FerroSeek index:

```sh
noctalia msg plugins disable lucenx9/ferroseek
noctalia msg plugins source remove ferroseek
```

## How it works

`plugin.toml` declares a `launcher_provider` implemented in `launcher.luau`.
Noctalia debounces input by 100 ms. The provider runs `ferroseek search --json
--no-hidden -n 30 -- <query>` through `noctalia.runAsync` with an argument
array. It keeps at most one search in flight, replaces queued queries with
the latest text and discards stale replies. Result order comes from FerroSeek.
Activation passes the selected absolute path directly to `xdg-open` as an
argument; neither search text nor file paths are interpolated into shell code.

Missing executables, absent/unreadable indexes, failed or truncated responses,
no matches and deleted files have distinct messages. The plugin does not
currently offer previews, copy-path or reveal-in-parent actions. Non-UTF-8
paths are skipped rather than opened using a lossy replacement name.

## CLI JSON contract (version 1)

`ferroseek search --json report` emits one JSON object and a newline:

```json
{
  "version": 1,
  "results": [
    {
      "path": "/example/report.txt",
      "name": "report.txt",
      "kind": "file",
      "size": 123,
      "mtime_ns": 1793491200000000000,
      "score": 78.5
    }
  ]
}
```

Results are in the same ranked order as the ordinary CLI. `kind` is `file`,
`directory`, `symlink` or `other`; `size` is bytes, and `mtime_ns` is an integer
Unix timestamp in nanoseconds. Consumers requiring exact nanoseconds must use
an integer-aware JSON parser (JavaScript/Luau numbers cannot represent every
64-bit integer exactly). The launcher only consumes paths, names and kinds.

Unix paths that are not UTF-8 have `path: null` and an additional `path_bytes`
array containing the exact absolute path bytes (0–255). `name` is display-only
and can contain Unicode replacement characters. Never use it to open a file.
UTF-8 paths omit `path_bytes`; JSON escapes quotes, control characters and
newlines normally. Raw-byte consumers can continue to use `search -0`.

Exit codes remain **0** for matches, **1** for no matches (a valid
`{"version":1,"results":[]}` response), and **2** for errors (diagnostic on
stderr, no result object). `--json` conflicts with `--long` and `--null`.
Existing CLI filters and query syntax are unchanged.

## Validation

```sh
cargo test -p ferroseek-cli --locked
lua5.4 tests/noctalia-provider.lua
noctalia plugins lint integrations/noctalia/ferroseek
```

CLI tests use a temporary index and a dead session bus. The real provider
script is exercised in the Lua/Luau common subset with a mocked Noctalia host:
query coalescing, stale responses, literal argument handling, scope, ranking,
activation, errors and unsupported paths. These tests never install a plugin
or open files on the desktop. Lint was also checked with Noctalia 5.4.1.
An interactive Noctalia launcher/xdg-open smoke test remains a manual check;
the mock-host tests do not establish end-to-end desktop compatibility.

API references: [launcher providers](https://docs.noctalia.dev/noctalia/plugins/development/entries/),
[subprocesses](https://docs.noctalia.dev/noctalia/plugins/development/runtime-api/),
[local plugin sources](https://docs.noctalia.dev/noctalia/plugins/development/workflow/).
