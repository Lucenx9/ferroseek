# Desktop reliability trial

Use a normal per-user installation with D-Bus activation (`--enable` is optional).
Open the FerroSeek launcher and configure the folders to index. File search activates the daemon,
which keeps watching those folders for the rest of the session. Start with
explicit document, download, desktop or picture folders. Leave Baloo and KDE
preferences unchanged.

## Independent comparison

Python 3.11+ is needed. The oracle enumerates the filesystem with Python's
lstat/scandir, applies byte-based name exclusions, hidden/cache-tag/depth
and mount policy, and reads the SQLite parent tree in one read transaction.
It does not use FerroSeek's search or scanner to generate expected results.
Directories are compared by path/kind; their stored mtime is a reconciliation
marker, not file metadata. Files/symlinks compare kind, size and nanosecond mtime.
File contents and FTS rankings are outside this check's scope.

```sh
umask 077
mkdir -p "$HOME/.local/state/ferroseek-soak"
ferroseek config > "$HOME/.local/state/ferroseek-soak/effective.toml"
python3 scripts/verify-index.py \
  --config "$HOME/.local/state/ferroseek-soak/effective.toml" \
  --database "$HOME/.local/share/ferroseek/index.sqlite"
```

Use the actual XDG index path reported by `ferroseek config` if different.
Refresh the effective TOML after changing configuration. The snapshot contains
private roots: keep it outside the repository. Output contains counts only.
Exit 0 means no discrepancies; 1 means discrepancies with stable before/after
filesystem enumerations; 2 means incomplete or concurrent filesystem activity.
An unstable comparison is not a pass. Double enumeration reduces races but
cannot make the entire filesystem atomic (including change-and-revert races).
Repeat discrepancies after activity has settled before diagnosing corruption.
Do not automatically run reindex before comparing: that would conceal defects.

The verifier holds snapshots in memory and does filesystem reads; its resource
use is separate from the lightweight daemon. Run hourly at low priority and
only while the daemon is active. Keep the config, index and local log private.
A finite systemd user timer with explicit hourly calendar entries over 72 hours
can collect observations without activating the daemon or starting it at login.
When the trial ends, stop/remove only the trial timer and its service. Keep
FerroSeek installed if it is useful; uninstall normally without `--purge` if not.

## Acceptance evidence

Collect at least three days of ordinary desktop work, including saves, renames,
moves, deletions, suspension/resume, logout/login and launcher file searches.
Record actual elapsed coverage, check counts, stable discrepancies and inconclusive scans.
Keep results as local counts; never publish filesystem names or screenshots.
Persistent discrepancies need investigation, not an unconditional rebuild.
A single successful initial comparison does not establish multi-day reliability.

## Recovery regressions

Run the seeded restart check with the release CLI and daemon.

```sh
python3 tests/index-recovery.py target/release
```

The test builds a synthetic tree on a private D-Bus session, verifies the initial
index, then creates, moves, renames and deletes files. It kills the daemon with
SIGKILL, makes further changes while it is down and requires the oracle to
detect drift. It restarts the daemon with the same index, waits for that process
to report idle and runs the oracle again. The check passes only on a stable match
after automatic recovery. The script prints the seed. Use `--seed NUMBER` to
repeat the filesystem operations. The kernel determines which live events have
reached the index when the daemon stops.

Deterministic tests inject EIO, ESTALE and EACCES at stat boundaries and EIO
inside directory iteration, exercising the real reconciliation code with real
SQLite/inotify/temp directories. They first failed on subtree deletion, missing
retry and stale metadata after overflow. They now assert preserved entries,
recovery without a new event, and metadata refreshed despite unchanged directory
mtime. Injection is compiled only for tests. Existing live permission and
create/delete overflow tests remain. This does not simulate every kernel or
network filesystem failure; inaccessible scan entries are revisited at the next
reconciliation (periodic by default), rather than causing a tight retry loop.
