# FerroSeek architecture

Status: MVP architecture, last updated 2026-10-31. Decisions are recorded
with the evidence that drove them. Anything marked *hypothesis* has not been
measured yet.

## Goals and non-goals

- **Goal:** filename and path search over user directories, kept current in
  near real time by inotify. It should have low, flat idle memory, run without
  root, and be reachable from KRunner through the official `org.kde.krunner1`
  D-Bus protocol, the same mechanism Baloo's own runner uses
  (`/usr/share/krunner/dbusplugins/plasma-runner-baloosearch.desktop`).
- **Goal:** plain-text content search (TXT/Markdown) as an opt-in second
  stage.
- **Non-goal for the MVP:** replacing the Baloo framework API (`Baloo::Query`,
  KIO `baloosearch:/`, tags, Dolphin integration). Those are roadmap items in
  `docs/ROADMAP.md`.

## Workspace layout

| crate | kind | responsibility |
|---|---|---|
| `crates/index` (`ferroseek-index`) | lib | SQLite store: schema, versioning and recovery, the tree mutation transaction, search and ranking |
| `crates/engine` (`ferroseek-engine`) | lib | config/XDG/exclusions, mount policy, scanner (reconciliation), inotify watcher, indexing pipeline (batching, pause, throttle, status) |
| `crates/daemon` (`ferroseekd`) | bin | process lifecycle, D-Bus control interface, KRunner runner |
| `crates/cli` (`ferroseek`) | bin | `search`, `status`, `pause`, `resume`, `index`, `rebuild`, `config` |
| `crates/bench` (`ferroseek-bench`) | bin | deterministic corpus generator and benchmark runner |

The brief proposed eight crates. The deletion test merged three of them:

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
- **Events.** inotify events map `wd → dir entry id` through an in-memory
  `HashMap<i32, (WatchDescriptor, i64)>` (about 32 B per watched directory).
  The path is recomputed from the database when the batch is applied, so it
  is always current. Inside one debounce window:
  - `IN_MOVED_FROM`/`IN_MOVED_TO` pairs with the same cookie become a tree
    rename. That is O(1), and the watch stays valid because inotify follows
    the inode.
  - An unpaired `MOVED_FROM` is a delete. An unpaired `MOVED_TO` is a create,
    plus a subtree scan if the entry is a directory.
  - Everything else marks `(dir_id, name)` dirty. Dirty entries are resolved
    with the primitive above after renames are applied.
- **Stale parents.** If a dirty name is missing *and* the parent's stored
  path no longer points at the same inode, the parent directory was moved by
  an event not yet processed. The parent is re-reconciled in a later batch
  instead of deleting data.
- **Overflow and suspend.** On `IN_Q_OVERFLOW`, after a pause, at startup,
  and on a timer (default every 6 h), a quick reconcile of every root runs.
  inotify keeps its queue across suspend, so suspend loses events only
  through overflow, which this covers.
- **Watch exhaustion.** If `inotify_add_watch` returns `ENOSPC`, the daemon
  keeps going in degraded mode. Those subtrees are covered only by periodic
  reconciliation, and `status` reports how many directories are unwatched.

## Threads and backpressure

```
inotify reader thread ──(sync_channel, 64 batches)──▶ indexer thread ──▶ SQLite (writer conn)
        ▲ kernel queue (16384 events)                   ▲ commands (pause/resume/reconcile/stop)
                                                        │
D-Bus (zbus zruntime helper thread) ── control iface ───┘
        └── KRunner Match ──▶ zruntime::unblock pool ──▶ SQLite (reader conns, WAL)
```

- Backpressure is end to end. When the indexer falls behind or is paused,
  the bounded channel fills, the reader thread blocks, and the kernel queue
  overflows. That raises `IN_Q_OVERFLOW`, which triggers a reconcile. Memory
  stays bounded for any burst size.
- The indexer runs at `nice 19` with the idle I/O class, set per thread, so
  query threads keep normal priority. Scans honour a duty-cycle limit
  (`max_cpu_percent`).
- KRunner queries run on `zruntime::unblock` pool threads, which exit after
  10 s idle. Reader connections live in a small pool and are dropped after
  60 s idle, so an idle daemon holds only the writer connection's bounded
  page cache.
- No tokio. zbus 6 drives itself with its own small runtime (`zruntime`), and
  everything else is plain threads. *Hypothesis:* idle RSS under 10 MiB and
  zero CPU wakeups at idle. To be measured in `docs/BENCHMARKS.md`.

## Search semantics and ranking

- The query is split on whitespace and lowercased. Every token must match:
  - A token of three or more characters is a substring of the name, using
    the FTS5 trigram index.
  - A token of one or two characters is a prefix of the name (`key` index)
    when it is the only token. Otherwise it is a substring filter on
    candidates.
  - A token containing `/` is a substring filter on the full path.
- Candidates are the union of exact-name matches, prefix matches and up to
  2000 FTS substring candidates. The cap bounds latency. Exact and prefix
  matches, the highest-ranked classes, are always included.
- **Score**, stage 1 (name only): exact name > exact stem > prefix >
  word-boundary match (`_ - . space`, camelCase) > inner substring, plus a
  coverage bonus and a penalty for dotfiles.
- **Score**, stage 2 (top 3×limit, with paths): a depth penalty, a penalty
  for hidden ancestors, and a recency bonus. The KRunner relevance is the
  score mapped into (0, 1).

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
  `XDG_ACTIVATION_TOKEN` comes from `SetActivationToken`. The "Open
  containing folder" action uses `org.freedesktop.FileManager1.ShowItems`.

## Security and privacy

- No network code and no telemetry.
- Data lives in `$XDG_DATA_HOME/ferroseek` (mode 0700, DB 0600) and config
  in `$XDG_CONFIG_HOME/ferroseek/config.toml`.
- Symlinks are never followed: entries are `lstat`ed, and a symlink is
  indexed as itself. Loops are therefore impossible, and the depth limit and
  ancestor check guard against bind-mount cycles.
- Mount policy: the daemon does not descend into virtual, network, FUSE or
  removable filesystems (`/proc/self/mountinfo` fstype) unless they are
  configured explicitly as roots.
