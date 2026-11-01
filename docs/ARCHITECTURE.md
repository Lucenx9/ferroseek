# FerroSeek architecture

Status: launcher and search engine architecture, updated 2026-11-01. Technical
decisions and historical measurements are retained with their evidence.

## Goals and non-goals

- **Goal:** a keyboard-first Linux launcher for applications, files and
  calculations, with local history and keyboard actions. The current UI
  targets Wayland with `wlr-layer-shell`.
- **Goal:** lightweight, desktop-independent filename and path discovery
  over configured Linux directories, kept current with inotify, without
  root privileges or mandatory graphical dependencies.
- **Goal:** one persistent search index powering the main launcher, CLI and optional
  integrations: KRunner for KDE Plasma, Noctalia's launcher and MCP for agents.
- **Future option:** bounded, opt-in text-content search measured separately.
- **Non-goal for the MVP:** replacing the Baloo framework API (`Baloo::Query`,
  KIO `baloosearch:/`, tags or Dolphin search integration). GNOME, Rofi and
  other launchers beyond Noctalia are deferred adapters, not currently implemented.

## Workspace layout

| crate | kind | responsibility |
|---|---|---|
| `crates/index` (`ferroseek-index`) | lib | SQLite store: schema, versioning and recovery, the tree mutation transaction, search and ranking |
| `crates/engine` (`ferroseek-engine`) | lib | config/XDG/exclusions, mount policy, scanner (reconciliation), inotify watcher, indexing pipeline (batching, pause, throttle, status) |
| `crates/daemon` (`ferroseekd`) | bin | process lifecycle, D-Bus control interface, KRunner runner |
| `crates/cli` (`ferroseek`) | bin | `search`, `status`, `pause`, `resume`, `index`, `rebuild`, `config` |
| `crates/mcp` (`ferroseek-mcp`) | bin | optional read-only stdio MCP adapter and access policy, independent of KDE |
| `crates/launcher` (`ferroseek-launcher`) | bin | main Wayland launcher: applications, daemon file results, calculator; never opens the index |
| `crates/bench` (`ferroseek-bench`) | bin | deterministic corpus generator and benchmark runner |

The original engineering brief proposed eight crates. Several of those
responsibilities were consolidated before adding the optional MCP adapter:

- **search → index.** The query SQL depends on the schema. A separate crate
  would be a pass-through that reaches into index internals.
- **scanner + watcher → engine.** Both do one job: make the stored tree match
  the filesystem. They share the exclusion rules, the tree lookups and the
  same state-based "re-examine (dir, name)" primitive. Separating them would
  duplicate that logic.
- **krunner → module in the daemon.** It has a single adapter (zbus), it runs
  in the daemon process, and its logic is search plus result encoding.
- **core → engine.** Config is consumed only by the engine, the daemon and
  the CLI.

## Storage: SQLite FTS5 trigram

Evidence is in `prototype/backend-bench/README.md`. At 1M names on Btrfs:
durable update p50 is 1.26 ms (vs 20.6 ms for Tantivy), query p99 is 2.1 ms
(vs 8.7 ms), and RSS growth is ~0 (vs 37 MiB). The cost is about 3× the disk
of Tantivy.

```sql
entries(id INTEGER PRIMARY KEY, parent INTEGER, name BLOB, key TEXT,
        kind INTEGER, size INTEGER, mtime INTEGER, ino INTEGER)
UNIQUE INDEX (parent, name)  -- tree navigation and upsert
INDEX (key)                  -- exact and prefix lookups (also for 1-2 char queries)
names USING fts5(key, content='', contentless_delete=1, tokenize='trigram')
meta(key, value)             -- schema version, clean-shutdown flag, etc.
```

- **Parent-pointer tree, not full paths.** Renaming a directory is a one-row
  update no matter how large its subtree is. Baloo also keeps an id tree
  (`idtree`/`idfilename`). A root is an entry with `parent IS NULL` whose
  `name` is its absolute path.
- **Paths are keyed by (parent, name), not by inode or device.** `ino` is used
  only for change detection. Baloo derives document IDs from device and inode,
  so changes in Btrfs device numbering invalidate its index (see
  RESEARCH.md). FerroSeek's identity survives them.
- **Names are raw bytes (`BLOB`).** Non-UTF-8 names round-trip exactly.
  `key` is the lowercased, lossily decoded form used for matching.
- **WAL with `synchronous=NORMAL`.** A process crash can never corrupt the
  index or lose a committed transaction. Power loss can drop the last few
  transactions, and startup reconciliation repairs that. `synchronous=FULL`
  costs about 1.3 ms per commit and buys nothing here, because the index is a
  rebuildable cache.
- **Recovery.** If opening fails with a corruption or "not a database" error,
  or the schema version is newer or incompatible, the file is renamed aside
  (`index.sqlite.broken`) and rebuilt from the filesystem. The index is a
  cache, so this never loses user data.

## Change detection: one state-based primitive

The scanner and the watcher both reduce to one operation: **re-examine
`(dir_id, name)`**. It `lstat`s the current path. If the path exists, the
entry is upserted. If it is gone, the entry and its subtree are deleted.
Because the operation reads current state rather than replaying event deltas,
it is idempotent, and races between the scan and events converge.

- **Reconcile a directory.** Add an inotify watch *first*, then `stat` the
  directory and `readdir`. Diff the listing against the stored children:
  insert, update or delete. In *quick* mode, a directory whose mtime is
  unchanged skips `readdir` and only descends into the child directories
  already known. *Full* mode always `readdir`s and stats every file.
- **Events.** inotify events map `wd → dir entry id` through two in-memory
  `HashMap`s, `i32 → i64` and its inverse, about 40 B per watched directory.
  The path is recomputed from the database when the batch is applied, so it
  is always current. inotify is bound directly through `libc`
  (`crates/engine/src/inotify.rs`) using a small syscall surface.
  Watches are installed only on selected indexed directories; inside one
  debounce window:
  - `IN_MOVED_FROM`/`IN_MOVED_TO` pairs with the same cookie become a tree
    rename. That is O(1), and the watch stays valid because inotify follows
    the inode.
  - An unpaired `MOVED_FROM` is a delete. An unpaired `MOVED_TO` is a create,
    plus a subtree scan if the entry is a directory.
  - A directory move that changes depth or intersects a path exclusion
    invalidates its subtree's listing timestamps in the rename transaction.
    Reconciliation rechecks the new policy, even after an interrupted scan
    and restart, and removes watches beyond the depth limit. Other directory
    renames retain the one-row tree update without a subtree scan.
  - Everything else marks `(dir_id, name)` dirty. Dirty entries are resolved
    with the primitive above after renames are applied.
- **Stale parents.** If a dirty name is missing *and* the parent's stored
  path no longer points at the same inode, the parent directory was moved by
  an event not yet processed. The parent is re-reconciled in a later batch
  instead of deleting data.
- **Lost events and suspend.** Queue overflow and discarded event batches
  trigger a full reconciliation; ordinary startup and periodic checks
  normally use the quicker directory-mtime pass. There is not yet a
  dedicated logind-triggered reconciliation on resume; unusual missed
  events remain a validation target.
- **Watch exhaustion.** If `inotify_add_watch` returns `ENOSPC`, the daemon
  keeps going in degraded mode. Unwatched subtrees depend on reconciliation;
  `status` reports `watch_limit_hit`. Per-root coverage is future work.

## Threads and backpressure

```
inotify reader thread ──(sync_channel, 64 batches)──▶ indexer thread ──▶ SQLite (writer conn)
        ▲ kernel queue (16384 events)                   ▲ commands (pause/resume/reconcile/stop)
                                                        │
D-Bus (zbus zruntime helper thread) ── control iface ───┘
        └── KRunner Match ──▶ zruntime::unblock pool ──▶ SQLite (reader conns, WAL)
```

- **Backpressure is end to end.**
  - When the indexer falls behind, the bounded channel fills, the reader
    thread blocks, and the kernel queue overflows. That raises
    `IN_Q_OVERFLOW`, which triggers a reconcile.
  - More than 100k pending events in one debounce window are dropped in
    favour of a reconcile.
  - While paused, events are discarded and a quick reconcile runs on resume.

  Memory stays bounded for any burst size. Measured: a 10k-file create,
  rename and delete burst, with 0 overflows and a peak RSS under 30 MiB at
  1M files.
- The indexer runs at `nice 19` with the idle I/O class, set per thread,
  so query threads keep normal priority. Scans honour a duty-cycle limit
  (`max_cpu_percent`).
- **Scan transactions** last 1 s or 200k row operations. That was measured
  on 1M files: 57 % fewer bytes written than 100 ms transactions, at no
  memory cost (`docs/BENCHMARKS.md` §4). Events that arrive during a slice
  wait at most 1 s.
- KRunner queries run on `zruntime::unblock` pool threads, which exit after
  10 s idle.
  - Reader connections live in a small pool (at most 4) that KRunner's
    `Teardown` empties at the end of every session.
  - `Status` uses a short-lived reader.
- **Idle release.** After 5 s without work, the engine:
  - checkpoints and truncates the WAL
  - frees the writer's page cache (`PRAGMA shrink_memory`)
  - calls `malloc_trim`

  The daemon caps glibc at 2 malloc arenas.
- The **daemon/engine** do not use Tokio: zbus 6 uses `zruntime`
  and indexing uses plain threads. The **optional MCP process** uses its
  own Tokio current-thread runtime and bounded blocking pool for stdio.
- **Measured (1M files):**
  - 0.000 % idle CPU and 0 context switches per second over 30 s
  - 9.2 MiB steady-state idle RSS after a restart (3.1 MiB anonymous)
  - about 26 MiB peak during the initial index

## Search semantics and ranking

- The query is split on whitespace and lowercased. Every token must match:
  - A token of three or more characters is a substring of the name, using
    the FTS5 trigram index.
  - A token of one or two characters is a prefix of the name (`key` index)
    when it is the only token. Otherwise it is a substring filter on
    candidates.
  - A token containing `/` is a substring filter on the full path.
- Candidates combine exact-name, prefix and FTS substring matches. Budgets
  apply after name, kind, hidden and path filters: 100 exact matches,
  `max(300, limit)` prefixes and `max(2000, 4 * limit)` FTS matches.
  Ranking is approximate when these budgets are reached.
- Path-only queries stream entries through the path filter without inventing
  a filename constraint. Slash-only queries return no matches. Path caches
  are cleared at 8192 entries (a single ancestor chain may exceed this).
  Selective filters and path-only queries can scan the whole candidate
  source; memory is bounded, but latency is not constant.
- **Score**, stage 1 (name only): exact name > exact stem > prefix >
  word-boundary match (`_ - . space`, camelCase) > inner substring, plus a
  coverage bonus and a penalty for dotfiles.
- **Score**, stage 2 (top 3×limit, with paths): a depth penalty, a penalty
  for hidden ancestors, and a recency bonus. The KRunner relevance is the
  score mapped into (0, 1).

## Adapter boundaries

The indexer and read-only `Reader` work without a desktop environment.
KRunner is a D-Bus adapter served by the daemon; MCP is an independent
stdio process which opens the *same* SQLite index read-only. Neither
creates or maintains a second index.

```text
CLI ────────────┐
KRunner (KDE) ──┼──> shared FerroSeek index <── inotify indexing engine
MCP (agents) ───┘
Noctalia ──> CLI (search --json) ──> same index
ferroseek-launcher ──> daemon D-Bus runner (Match/Run) ──> same index
```

The main `ferroseek-launcher` is a separate layer-shell client. It gets
file results from the daemon's runner over D-Bus, parses desktop entries for
applications itself and keeps its own launch history. See
[LAUNCHER.md](LAUNCHER.md).

The optional Noctalia v5 adapter lives in `integrations/noctalia/ferroseek`.
It uses argument-array subprocesses, coalesces input while a query is in flight,
and opens results through `xdg-open`. See [setup and validation limits](NOCTALIA.md).
New GNOME, Rofi and other desktop adapters are deferred while the standalone launcher is hardened.

## KRunner integration

- `~/.local/share/krunner/dbusplugins/ferroseek.desktop`: `X-Plasma-API=DBus`,
  service `io.github.ferroseek.FerroSeek`, path `/runner`.
- D-Bus activation goes through
  `~/.local/share/dbus-1/services/io.github.ferroseek.FerroSeek.service`, with
  `SystemdService=ferroseek.service`. KRunner can start the daemon on
  demand, and systemd manages its lifecycle.
- The match id is a percent-encoded `file://` URL, valid UTF-8 even for
  non-UTF-8 paths. `Run` decodes it, checks that the path is in the index,
  and opens it with `xdg-open <path>` as a single argv entry (no shell).
  The opener runs inside `systemd-run --user --scope` when a user manager
  is available, so applications never become part of `ferroseek.service`
  and its limits, restarts or `KillMode` cannot touch them. Verified live:
  Gwenview ran in its own `app-…service`. `XDG_ACTIVATION_TOKEN` comes from
  `SetActivationToken`. The "Open containing folder" action uses
  `org.freedesktop.FileManager1.ShowItems`.
- `X-Plasma-API=DBus` rather than `DBus2`: with `DBus2`, KRunner suspends
  matching until `Config` replies, and FerroSeek has no runtime config to
  report.
- **Installation note found in live testing.** dbus-broker (Arch's session
  bus) does not watch service directories, so the installer calls
  `org.freedesktop.DBus.ReloadConfig`. Without it, KRunner gets "name is not
  activatable".

## Security and privacy

- No network search service or telemetry. The core and MCP interfaces are local.
- Data lives in `$XDG_DATA_HOME/ferroseek` (mode 0700, DB 0600) and config
  in `$XDG_CONFIG_HOME/ferroseek/config.toml`.
- Symlinks are never followed: entries are `lstat`ed, and a symlink is
  indexed as itself. Loops are therefore impossible, and the depth limit and
  ancestor check guard against bind-mount cycles.
- Mount policy: the daemon does not descend into virtual, network, FUSE or
  removable filesystems (`/proc/self/mountinfo` fstype) unless they are
  configured explicitly as roots.

## Optional MCP server (`crates/mcp`, `ferroseek-mcp`)

- **Separate binary, stdio only**, using the official `rmcp` 3.8 SDK
  (features `server`, `macros`, `transport-io`; no HTTP or auth code). It is
  a child process of the agent and exits on EOF, so it costs nothing when
  no agent runs it.
- **Read-only.** It reuses `ferroseek_index::Reader` against the daemon's
  database. It adds two opt-in items to the index crate:
  - `Reader::roots()`, a cheap read of the root rows.
  - `SearchOptions::accept`, a path predicate applied with the index's own
    filters, before candidate budgets. `None` everywhere else; the index
    schema is unchanged. It caches one connection, keyed by the
  database's device and inode, and drops it plus trims the heap 5 s after
  the last request.
- **Policy inside the budget.** The MCP passes its access policy as
  `accept`, memoized per parent directory, so denied entries can neither
  starve results nor be inferred. This came from an independent review.
- **Policy before output.** New unconfigured MCP instances default to
  `all_indexed`, covering only *already indexed* paths. Legacy explicit
  allowlists retain `restricted` behavior; users can explicitly select
  `restricted` or `home`. The selected mode never changes indexing roots. Hidden and sensitive-looking
  components remain filtered by default. Paths containing `..` or `.`
  are refused. Policy checks occur before candidate budgets and again
  before output. See `docs/MCP.md` for scope and latency details.
- **Bounded output.** Results are capped by count (≤ 200) and serialized
  bytes (24 KiB). Stale results are dropped after a race-safe
  `openat`/`fstatat` walk that never follows symlinks below the allowed
  directory (`beneath.rs`, a review fix).
- Details, verification and measurements are in `docs/MCP.md` and
  BENCHMARKS.md §10.

## Review hardening (2026-10-31)

The inotify reader now returns after one buffer, letting the bounded channel
apply backpressure between reads. Cache-tag exclusions also apply to event
resolution; removing or invalidating a tag schedules a full directory scan.

An opaque key of resolved rules and roots is stored in SQLite. A changed
key atomically invalidates all reconciled directory mtimes before storing
the new key. Interrupted scans leave unvisited directories invalid, so a
restart completes the policy transition. The first launch after this change
also revisits directory listings. No schema version change is needed.

## Bounded directory scopes (optimization pass)

Path filtering reuses `names` FTS5 to locate a necessary directory component
before the last slash. It then follows the existing `entries_parent_name`
index, never materializing full paths in persistent storage. Raw paths are
still constructed and checked for every accepted candidate. There is no
new schema, persistent index, or directory-rename write amplification.

Scope selection falls back to the original strategy for root anchors or
more than 256 seed/descendant directories. Recursive traversal explicitly
uses the parent index: SQLite otherwise chose an expensive automatic index
in profiling. Scopes with at most 4096 entries are searched directly; larger
scopes combine the existing filename strategy with SQL parent filtering.
Prefix candidates retain key/id ordering before their budget is applied.
The entire search holds one SQLite read snapshot.

The bounded probe does not guarantee constant latency: very wide directories
and short directory components remain potential scan costs. Candidate
ranking remains approximate for large result sets, as documented previously.
