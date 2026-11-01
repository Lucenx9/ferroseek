# Noctalia launcher integration

The optional `lucenx9/ferroseek` plugin adds filename/path search to Noctalia
v5's existing launcher. Type `/ferro report`, select a result with the arrow
keys and press Enter to open it with the default application. Directories
open in the file manager. No KDE, KRunner or MCP server is needed.

## Requirements and installation

- Stable Noctalia v5+ with plugin API **24 or newer** (argument-array subprocesses).
  Noctalia v4/QML plugins use a different API and are not supported.
- The FerroSeek CLI with `search --json` support (installed with the plugin).
- GLib's `gio` command and a configured default file manager/applications
  (`glib2` on Arch, `libglib2.0-bin` on Debian/Ubuntu).

From a source checkout or a matching Noctalia/full native archive:

```sh
./scripts/install.sh --noctalia --dry-run
./scripts/install.sh --noctalia
```

The installer checks Noctalia and `gio`, installs core plus a managed plugin
copy under `$XDG_DATA_HOME/noctalia/plugins/ferroseek` (normally
`~/.local/share/noctalia/plugins/ferroseek`) and reloads Noctalia when reachable.
It sets the plugin's default executable to the installed CLI's absolute path,
so Noctalia need not inherit `~/.local/bin` on PATH. Existing plugin setting
overrides remain in effect. It never installs system dependencies itself.

Review `ferroseek config` and choose indexing roots/exclusions **before**
starting indexing; the engine's default root is the home directory. Activate
the plugin alone after installation:

```sh
noctalia msg plugins enable lucenx9/ferroseek
```

For live indexing and plugin activation together, explicitly opt in:

```sh
./scripts/install.sh --noctalia --enable
```

This needs a running Noctalia instance plus the systemd user manager/session
bus. It enables/starts the indexer and enables the plugin through Noctalia's
native IPC. Upgrades never restart an active indexer. Without `--enable`, the
installer preserves Noctalia's saved enabled list; an already enabled plugin
continues to use the updated files. Searching also works against an index
created with `ferroseek index` while the daemon is stopped.

## Updates, removal and development

Re-run `./scripts/install.sh --noctalia` from the new source/archive to update
the copied plugin and core. The installer owns each copied file by its hash,
uses the same rollback journal as other components and refuses unrelated or
locally modified files. The installed plugin remains usable if the source
checkout is moved or removed. `--full` and interactive recommendations include
Noctalia when stable Noctalia and `gio` are detected.

```sh
./scripts/uninstall.sh --noctalia --dry-run
./scripts/uninstall.sh --noctalia
```

Removal disables the plugin through IPC when Noctalia is running, then removes
only managed plugin files. Core, index and settings are retained. Offline
removal retains Noctalia's saved enabled list; disable the entry in Settings
if needed, especially if another source supplies the same plugin ID.

For **development**, a path source still allows editing in place:

```sh
# Run from the FerroSeek checkout; keep this checkout at this location.
noctalia msg plugins source add ferroseek path "$PWD/integrations/noctalia"
noctalia msg plugins enable lucenx9/ferroseek
```

The managed local-data copy takes precedence over a path/git source with the
same ID. When switching from a development source, keep existing settings;
you can remove that source explicitly with `noctalia msg plugins source remove
ferroseek`. Removing a source can also disable its plugin ID, even when the
managed local copy overrides it. Re-enable the managed copy afterwards with
`noctalia msg plugins enable lucenx9/ferroseek`. The installer never removes
user-defined sources automatically.
FerroSeek is not in Noctalia's community catalog.

Open the launcher with your existing keybinding (for example Super+Space on
Niri) and type `/ferro report`. `/` lists the provider alongside built-ins.

In **Noctalia Settings → Plugins → FerroSeek**, configure:

| Setting | Default | Meaning |
|---|---|---|
| FerroSeek executable | Installed CLI absolute path; `ferroseek` in development source | Executable name on PATH, or an absolute/`~/` path |
| Search directory | empty | Optional `--in` scope inside the existing index; it does not authorize or index a new root |
| Maximum results | 30 | Between 1 and 100 |

The provider only participates behind `/ferro`. It does not add file results
to ordinary app searches. Hidden paths are always excluded by `--no-hidden`.
This is a local desktop adapter using the CLI: **MCP allowlists and sensitive
name filters do not apply**. Other non-hidden indexed paths are searchable,
subject to the optional search-directory scope.

## How it works

`plugin.toml` declares a `launcher_provider` implemented in `launcher.luau`.
Noctalia debounces input by 100 ms. The provider runs `ferroseek search --json
--no-hidden -n 30 -- <query>` through `noctalia.runAsync` with an argument
array. It keeps at most one search in flight, replaces queued queries with
the latest text and discards stale replies. Result order comes from FerroSeek.
Activation passes the selected absolute path directly to `gio open` as an
argument; neither search text nor file paths are interpolated into shell code.
GIO reports completion after handing the request to the default application.
The callback checks that handoff's exit status, not the application's lifetime.
Some `xdg-open` desktop backends wait until the launched application exits;
capturing those calls with Noctalia's default five-second process timeout can
produce a false failure notification and terminate the application's process
group even though it opened successfully. The provider therefore requires GIO.

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
python3 tests/noctalia-open-smoke.py
python3 tests/noctalia-install-smoke.py
noctalia plugins lint integrations/noctalia/ferroseek
```

CLI tests use a temporary index and a dead session bus. The real provider
script is exercised in the Lua/Luau common subset with a mocked Noctalia host:
query coalescing, stale responses, literal argument handling, scope, ranking,
activation, errors and unsupported paths. These tests never install a plugin
or open files on the desktop. Lint was also checked with Noctalia 5.4.1.
The GIO smoke test uses isolated desktop associations and a fake application
that stays alive beyond Noctalia's default timeout. It verifies literal path
delivery, prompt handoff and actual launch errors without opening a desktop
application. A live check on Arch Linux, Niri and Noctalia 5.4.1 opened a
directory from `/ferro` in a fresh Nautilus process; its window remained open
beyond the five-second timeout, without an error notification. Other desktop
and file-type combinations still need manual validation.

API references: [launcher providers](https://docs.noctalia.dev/noctalia/plugins/development/entries/),
[subprocesses](https://docs.noctalia.dev/noctalia/plugins/development/runtime-api/),
[local plugin sources](https://docs.noctalia.dev/noctalia/plugins/development/workflow/).
