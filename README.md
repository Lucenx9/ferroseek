# FerroSeek

A lightweight, real-time **file name search engine for Linux**, written in
Rust, with a **native KRunner integration** for KDE Plasma 6.

FerroSeek keeps an index of the files and folders under the directories you
choose and updates it live through inotify. You can search it from KRunner
(Alt+Space) or from the `ferroseek` command line. It is an independent
daemon: it does not replace, modify or depend on KDE Baloo.

> **Status: MVP (0.1).** File and folder *name* search, live updates,
> KRunner and the CLI work, and are tested and benchmarked
> (`docs/BENCHMARKS.md`). Content search, Dolphin integration and Baloo API
> compatibility are **not** implemented yet (see `docs/ROADMAP.md`).

## Why

Baloo, KDE's indexer, is a capable full-text and metadata indexer. Its
filename search has some structural costs, documented with source citations
in `docs/RESEARCH.md`:
- It matches word prefixes only (bug 333037).
- Its document IDs derive from the device and inode, which breaks on Btrfs
  ID changes (bug 525145).
- An inotify overflow is ignored (bug 437754).
- Posting lists are rewritten on every commit (bug 442453).

FerroSeek targets the narrower filename/path use case with a different
design:

- **Substring matching**, case-insensitive and Unicode-aware, using a SQLite
  FTS5 trigram index. It ranks exact name > stem > prefix > word boundary >
  inner match, then adjusts for depth and recency.
- **Path-based identity.** Entries are keyed by (parent, name), not by inode
  or device. Renaming a directory updates one row, whatever its size.
- **Low idle resource use.** In the latest 1M-file run, idle RSS after
  restart was 10.1 MiB; a 30-second idle window recorded no CPU time or
  context switches. Mixed KRunner queries measured 2.48 / 24.80 / 27.48 ms
  p50 / p95 / p99. Five paired runs reduced a selective path-only query
  from 530 ms to 2.03 ms without adding a persistent index. See
  [measurements and limits](docs/BENCHMARKS.md#9-path-optimization-2026-10-31).
- **Recovery from missed events.** After inotify overflow, pending-event overflow or dropped events during
  pause, it performs a full reconciliation, including file metadata. Startup
  and periodic reconciliation normally use a quick directory-mtime pass.
- **Crash-safe.** It uses SQLite WAL. A corrupt or incompatible index is
  moved aside and rebuilt automatically. Recovery after `SIGKILL` was
  measured to be complete.

No benchmark against Baloo has been run yet. See
[docs/BENCHMARKS.md §6](docs/BENCHMARKS.md) for the protocol and the reasons
the two are not directly comparable. **This project does not claim to be
faster than Baloo.**

## Build

You need Rust 1.89 or newer. SQLite is compiled in (bundled), so there are
no system library requirements beyond libc.

```sh
cargo build --release
```

This builds `target/release/ferroseekd` (the daemon) and
`target/release/ferroseek` (the CLI).

## Install (per user, no root)

```sh
scripts/install.sh            # build + install to ~/.local
scripts/install.sh --enable   # …and start indexing now via systemd --user
kquitapp6 krunner             # make KRunner pick up the new runner
```

It installs:

| file | purpose |
|---|---|
| `~/.local/bin/ferroseekd`, `~/.local/bin/ferroseek` | binaries |
| `~/.local/share/krunner/dbusplugins/ferroseek.desktop` | KRunner D-Bus runner registration |
| `~/.local/share/dbus-1/services/io.github.ferroseek.FerroSeek.service` | D-Bus activation: KRunner starts the daemon on first use |
| `~/.config/systemd/user/ferroseek.service` | systemd user unit (`Type=dbus`, `background.slice`, resource limits) |

The installer also asks your session bus to reload its service files, which
dbus-broker needs before a new service can be activated. It never touches
system files, Baloo, or KDE configuration.

To uninstall, run `scripts/uninstall.sh`. Add `--purge` to also delete
FerroSeek's index and config.

## Usage

```sh
ferroseek status                     # daemon state, counts, index size
ferroseek search invoice 2024        # all words must match (substring, any case)
ferroseek search -n 50 --long report # kind, size, mtime, score
ferroseek search --in ~/Documents cv # restrict to a directory
ferroseek search src/ main.rs        # words containing '/' match the full path
ferroseek search -0 .pdf | xargs -0 ls -l
ferroseek pause | resume             # stop/restart indexing (queries keep working)
ferroseek index [--full]             # reconcile now (foreground if no daemon)
ferroseek rebuild                    # drop and rebuild the index
ferroseek config [--default]         # effective configuration and file locations
```

Searching reads the index directly, so it also works while the daemon is
stopped. Queries of one or two characters match name prefixes; longer
queries match substrings.

### KRunner

Type at least two characters. Results appear grouped by category: Folders,
Documents, Images, Audio, Videos, Archives, Files.

- **Enter** opens the file with its default application. FerroSeek runs
  `xdg-open` in its own transient systemd scope, with no shell involved.
- **Open Containing Folder** shows the file selected in your file manager.
- Drag and drop works, because results carry `file://` URLs.

**Avoiding duplicate results with Baloo.** If Baloo's own "File Search"
runner is enabled, both runners return files. They de-duplicate matches with
the same URL, but you may prefer to disable one of them. FerroSeek never
changes this for you. To do it yourself:

- **System Settings → Search → Plasma Search**: untick **File Search**.
  KRunner's other plugins (apps, calculator, windows…) are unaffected.
- Or from a terminal: `kwriteconfig6 --file krunnerrc --group Plugins --key baloosearchEnabled false`.

Disabling the runner does not disable Baloo's indexer. That is a separate
choice, made in **System Settings → Search → File Search**.

## Configuration

`~/.config/ferroseek/config.toml` (all keys optional; `ferroseek config
--default` prints the defaults):

```toml
roots = ["~"]                 # directories to index
index_hidden = false          # dotfiles and dot-directories
exclude_names = [".git", "node_modules", "*.o", "*~", …]   # name globs (* ?)
exclude_paths = ["~/VMs"]     # absolute subtrees to skip
respect_cachedir_tag = true   # skip dirs with a CACHEDIR.TAG (e.g. Cargo target/)
stay_on_filesystem = true     # don't descend into other mounts below a root
max_depth = 64
max_cpu_percent = 50          # scan duty cycle (indexing thread is also nice 19 + idle I/O)
low_priority = true
debounce_ms = 200             # event coalescing window
reconcile_interval_minutes = 360
```

Unknown keys are rejected so that typos don't go unnoticed. Restart the
daemon after editing (`systemctl --user restart ferroseek`).

The index lives in `~/.local/share/ferroseek/index.sqlite`; the directory
is mode 0700 and the file 0600. `FERROSEEK_CONFIG`, `FERROSEEK_DATA_DIR` and
`FERROSEEK_LOG` (error/warn/info/debug) override the defaults. Logs go to
the journal: `journalctl --user -u ferroseek`.

## Limitations

- Search retains a bounded candidate set after filtering; ranking is
  approximate on very broad queries. Selective paths use existing directory
  indexes; broad anchors and components shorter than three characters may
  still scan many rows. Mixed-query p95 remains above the 10 ms target.

- Name and path search only, no file contents yet. Plain-text content
  search is the next milestone.
- There is no Dolphin, KIO (`baloosearch:/`) or `Baloo::Query`
  compatibility, and no tags, ratings or metadata.
- `inotify` needs one watch per directory. If `fs.inotify.max_user_watches`
  runs out, `ferroseek status` reports `watch_limit_hit: true` and the
  unwatched directories are refreshed only by periodic reconciliation.
- A quick reconcile after downtime detects new, deleted and renamed entries.
  Size and mtime changes of existing files inside unchanged directories are
  picked up live, or by `ferroseek index --full`.
- Network, FUSE and removable filesystems below a root are not descended
  into by default.
- Memory scales with directory/watch count and the width of directories
  reconciled; the measured RSS is not a universal memory bound.
- The installer rejects HOME paths containing characters other than ASCII
  letters, digits, underscore, dot, slash and hyphen.
- Benchmarks include repeated synthetic datasets and a real `/usr/include`
  oracle. Warm-cache and advisory-eviction measurements are distinguished;
  full cold-machine behavior and non-glibc targets remain unverified. See
  [release readiness](docs/RELEASE_READINESS.md).

## Agents (optional MCP server)

`ferroseek-mcp` is a separate, opt-in binary. It lets MCP clients such as
Claude Code and Codex search the existing index through three read-only
tools: `search_files`, `get_file_info` and `index_status`.

- It uses stdio only, starts no indexer and opens the database read-only.
- Agents see nothing unless you allow specific directories. Dotfiles and
  secret-looking names stay hidden.
- It runs only while an agent uses it.

```sh
cargo build --release -p ferroseek-mcp && install -Dm755 target/release/ferroseek-mcp ~/.local/bin/
claude mcp add --scope user ferroseek -- ~/.local/bin/ferroseek-mcp --allow ~/Progetti
codex mcp add ferroseek -- ~/.local/bin/ferroseek-mcp --allow ~/Progetti
```

See [docs/MCP.md](docs/MCP.md) for the access policy, configuration,
verification and measured performance. At 1M files, a search has a 3.2 ms
p50, and an idle server uses 0 % CPU and about 11 MiB RSS.

## Development

See `CLAUDE.md` for the repository map and conventions, and
`docs/ARCHITECTURE.md` for the design and data flow.

```sh
cargo build --release --workspace                       # includes benchmark binary
cargo test --workspace                                  # isolated unit/integration tests
tests/dbus-smoke.sh target/release/ferroseekd           # daemon on a private D-Bus session
tests/mcp-smoke.py                                      # MCP server over stdio, temp index
target/release/ferroseek-bench --files 100000 --work /tmp/fsb   # benchmarks
```

## Privacy and security

- No network code, no telemetry.
- The daemon runs unprivileged and never follows symlinks.
- `Run` only opens paths that are present in the index.

## License and attribution

FerroSeek is licensed under **GPL-3.0-or-later** (see `LICENSE`). All
dependencies use permissive licenses (MIT/Apache-2.0/BSD/Zlib), compatible
with it.

No KDE source code is included. The KRunner integration implements the
`org.kde.krunner1` D-Bus interface as documented in KRunner's
`kf6_org.kde.krunner1.xml` (LGPL-2.0-or-later, © David Edmundson, Kai Uwe
Broulik, Alexander Lohnau), and the default exclusion list was written
independently. Baloo (LGPL-2.1-or-later) was studied as a reference; all
findings are cited in `docs/RESEARCH.md`.
