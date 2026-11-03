# FerroSeek benchmarks

Sections 1–11 describe measurements on the machine below with the commands shown.
Section 12 uses a separate CI environment, identified alongside its results.
Nothing has been extrapolated. **No Baloo comparison has been run yet.**
Baloo is disabled on this machine, and the owner asked that it stay
disabled. The protocol for a controlled comparison is in
[§6](#6-baloo-comparison-protocol-not-yet-executed).

## 1. Environment

| | |
|---|---|
| CPU | AMD Ryzen 7 5800X (8 cores / 16 threads) |
| RAM | 31 GiB |
| Storage | KINGSTON SNV3S1000G NVMe SSD |
| Filesystem | Btrfs, `noatime,compress=zstd:1,ssd,discard=async` (`/home`) |
| OS / kernel | CachyOS, Linux 7.3.0-4-cachyos |
| Desktop | KDE Plasma 6.8.2, KF 6.31.0, Wayland |
| Toolchain | rustc 1.94.0, release profile (thin LTO, `codegen-units=4`) |
| Libraries | SQLite 3.54.1 (bundled via libsqlite3-sys 0.39.1, rusqlite 0.41.1), zbus 6.2.0 |
| FerroSeek | commit `6ee1eeb` |
| Date | 2026-10-31 |

## 2. Method

```sh
cargo build --release --workspace
target/release/ferroseek-bench --files 10000   --work ~/.cache/ferroseek-bench-10000
target/release/ferroseek-bench --files 100000  --work ~/.cache/ferroseek-bench-100000
target/release/ferroseek-bench --files 1000000 --work ~/.cache/ferroseek-bench-1000000
```

`ferroseek-bench` (`crates/bench/src/main.rs`) works as follows:

1. **Corpus.** It generates a deterministic corpus (xorshift, seed 42) on the
   real disk, not on tmpfs:
   - About 20 files per directory, with a skew toward a few large
     directories, and depth up to 10.
   - Names are a mix of camera, CamelCase, `word_N`, long (6–25 words) and
     Unicode names. About 5 % are dotfiles (excluded by default) and 0.1 %
     are non-UTF-8 names.
   - There are `node_modules`/`.git` trees that must be skipped.
   - Sparse file sizes from 0 B to 1 MiB (`set_len`, so no data is written).
2. **Daemon.** It re-executes itself under `dbus-run-session`, so it uses a
   **private session bus**. It starts the **release `ferroseekd`** with an
   isolated config and data dir, with the default configuration except for
   the root and `max_cpu_percent`. The user's bus, home and index are never
   touched.
3. **Metrics.** It measures through `/proc/<pid>/{status,io,stat,task}` and
   through the daemon's D-Bus interfaces, as KRunner sees them.

Definitions:

- **Initial index, wall.** Time from daemon start until `Status` reports
  `idle` with no queued directories. That includes adding the inotify
  watches.
- **Disk read / written.** `read_bytes` / `write_bytes` from
  `/proc/pid/io`, the bytes submitted to the block layer. Btrfs zstd
  compression can make actual device writes lower.
- **Idle.** Measured 12 s after the daemon goes idle (the query-pool threads
  expire after 10 s), over a 30 s window:
  - RSS from `VmRSS`, split into `RssAnon` and `RssFile`. File-backed pages
    are mostly shared libraries.
  - CPU from utime+stime.
  - "Context switches" is the sum over all threads of voluntary plus
    involuntary switches, a proxy for wakeups.
- **Query latency.** 390 deterministic queries:
  - 300 random 3–8 character substrings of real corpus names
  - 30 two-word queries, 30 two-character queries, 30 no-hit queries
  - "library" is `Reader::search` in-process; "D-Bus `Match`" is a full
    `org.kde.krunner1.Match` round trip over the bus. Both are warm (page
    cache hot).
- **Relevance.** 200 random corpus files:
  - Is the exact name ranked #1?
  - Is the stem in the top 5? Counted as a success if the file is in the top
    5, or if the top 5 are all exact stem ties: some synthetic stems exist
    hundreds of times.
  - Are its first two words (multi-word AND) found at all?
- **Change → visible.** Time from the filesystem call until `Match` returns
  (or stops returning) the file. This is dominated by the default 200 ms
  event debounce (`debounce_ms`).
- **Recovery.** `SIGKILL` once a third of the files are committed during a
  fresh initial index, then restart. "Complete" means the entry count equals
  that of an uninterrupted run.

## 3. Results

### 10000 files (seed 42)

Corpus: 10000 files in 500 directories (+300 files in excluded `node_modules`/`.git` trees), generated in 0.2s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 0.11 s (91148 files/s) |
| ↳ CPU time (user+sys) | 0.09 s |
| ↳ peak RSS (VmHWM) | 12.4 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 5.4 MiB |
| ↳ indexed files / dirs / watched dirs | 9505 / 510 / 510 |
| initial index, max_cpu_percent=100: wall | 0.11 s (91526 files/s) |
| ↳ CPU time (user+sys) | 0.08 s |
| ↳ peak RSS (VmHWM) | 11.5 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 5.3 MiB |
| ↳ indexed files / dirs / watched dirs | 9505 / 510 / 510 |
| index size on disk: database / WAL | 2.4 MiB (237 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 8.3 MiB (2.1 MiB anon, 6.2 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.15 / p95 0.52 / p99 0.66 / max 0.81 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.36 / p95 0.73 / p99 0.89 / max 1.32 ms |
| RSS after query burst | 11.2 MiB |
| relevance (sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |
| change → visible in `Match`: create (debounce 200 ms) | p50 202.44 / p95 203.14 / p99 277.50 / max 277.50 ms |
| ↳ rename | p50 202.17 / p95 206.13 / p99 207.88 / max 207.88 ms |
| ↳ delete | p50 201.83 / p95 203.05 / p99 203.11 / max 203.11 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.17 s after last write |
| burst: rename 10000 files (0.19 s) → consistent | 0.37 s after last rename |
| rename directory holding 10000 files → visible under new path | 265 ms |
| delete tree of 10000 files (0.17 s) → gone from index | 0.37 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 17.0 MiB / 12.2 MiB → 16.7 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 15.9 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.06 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 7.2 MiB (1.0 MiB / 6.2 MiB), 9.2 MiB |
| recovery: SIGKILL after 0.09 s (9505 files committed) → restart → complete | 0.06 s; 9505/9505 files ✔ |

### 100000 files (seed 42)

Corpus: 100000 files in 5000 directories (+3000 files in excluded `node_modules`/`.git` trees), generated in 1.7s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 2.96 s (33815 files/s) |
| ↳ CPU time (user+sys) | 1.70 s |
| ↳ peak RSS (VmHWM) | 16.9 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 89.8 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| initial index, max_cpu_percent=100: wall | 1.07 s (93834 files/s) |
| ↳ CPU time (user+sys) | 1.08 s |
| ↳ peak RSS (VmHWM) | 23.4 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 76.5 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| index size on disk: database / WAL | 25.1 MiB (251 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 8.9 MiB (2.6 MiB anon, 6.4 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.57 / p95 3.04 / p99 3.25 / max 4.09 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.75 / p95 3.00 / p99 3.32 / max 4.33 ms |
| RSS after query burst | 12.9 MiB |
| relevance (sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |
| change → visible in `Match`: create (debounce 200 ms) | p50 202.65 / p95 203.65 / p99 208.44 / max 208.44 ms |
| ↳ rename | p50 202.90 / p95 203.63 / p99 216.81 / max 216.81 ms |
| ↳ delete | p50 202.30 / p95 211.32 / p99 379.48 / max 379.48 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.18 s after last write |
| burst: rename 10000 files (0.19 s) → consistent | 0.27 s after last rename |
| rename directory holding 10000 files → visible under new path | 209 ms |
| delete tree of 10000 files (0.18 s) → gone from index | 0.34 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 21.9 MiB / 23.4 MiB → 23.6 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 23.1 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.14 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 11.5 MiB (5.4 MiB / 6.2 MiB), 20.0 MiB |
| recovery: SIGKILL after 1.04 s (95116 files committed) → restart → complete | 0.27 s; 95200/95200 files ✔ |

### 1000000 files (seed 42)

Corpus: 1000000 files in 50000 directories (+30000 files in excluded `node_modules`/`.git` trees), generated in 22.3s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 35.11 s (28480 files/s) |
| ↳ CPU time (user+sys) | 25.95 s |
| ↳ peak RSS (VmHWM) | 22.4 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 1628.8 MiB |
| ↳ indexed files / dirs / watched dirs | 953595 / 51000 / 51000 |
| initial index, max_cpu_percent=100: wall | 18.35 s (54486 files/s) |
| ↳ CPU time (user+sys) | 22.26 s |
| ↳ peak RSS (VmHWM) | 22.4 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 1697.1 MiB |
| ↳ indexed files / dirs / watched dirs | 953595 / 51000 / 51000 |
| index size on disk: database / WAL | 250.7 MiB (250 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 11.5 MiB (4.8 MiB anon, 6.7 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 2.32 / p95 6.36 / p99 8.38 / max 10.29 ms |
| query latency, D-Bus `Match` end-to-end | p50 2.43 / p95 6.38 / p99 8.18 / max 9.40 ms |
| RSS after query burst | 15.6 MiB |
| relevance (sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |
| change → visible in `Match`: create (debounce 200 ms) | p50 202.95 / p95 207.79 / p99 237.08 / max 237.08 ms |
| ↳ rename | p50 203.07 / p95 214.35 / p99 215.36 / max 215.36 ms |
| ↳ delete | p50 202.20 / p95 203.79 / p99 302.72 / max 302.72 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.19 s after last write |
| burst: rename 10000 files (0.19 s) → consistent | 1.31 s after last rename |
| rename directory holding 10000 files → visible under new path | 384 ms |
| delete tree of 10000 files (0.17 s) → gone from index | 0.36 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 27.9 MiB / 22.4 MiB → 27.2 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 59.9 MiB; 1 |
| restart on unchanged tree → idle (quick reconcile + watches) | 1.06 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 9.2 MiB (3.1 MiB / 6.1 MiB), 24.8 MiB |
| recovery: SIGKILL after 5.43 s (348280 files committed) → restart → complete | 15.24 s; 953595/953595 files ✔ |

### Summary (1M files)

| | FerroSeek |
|---|---|
| Initial index, unthrottled / default 50 % duty cycle | 18.3 s / 34.9 s |
| Peak RSS during initial index | ≈ 26 MiB |
| Database size | 251 MiB (250 B per entry), WAL truncated after each pass |
| Idle CPU / wakeups | 0.000 % / 0 per second (30 s window) |
| Idle RSS, steady state after restart | 9.2 MiB (3.1 MiB anonymous) |
| KRunner `Match` latency p50 / p99 | 2.4 ms / 8.2 ms |
| Restart reconcile (unchanged tree, 51k directories) | ≈ 1 s |

## 4. Tuning experiments (1M files, `--only-initial`, `max_cpu_percent=100`)

Initial-index write volume was the one metric that grew faster than linearly
(3.9 GiB written to build the 1M-file index). Experiments were run with
temporary knobs, which have since been removed:

| variant | wall | written | peak RSS | decision |
|---|---|---|---|---|
| baseline: 100 ms scan transactions | 23.5 s | 3901 MiB | 27.2 MiB | – |
| 1 s / ≤200k-op scan transactions | 17.8 s | 1661 MiB | 26.9 MiB | **adopted** |
| page cache 32 MiB (100 ms transactions) | 17.1 s | 2925 MiB | 77.9 MiB | rejected: +50 MiB RSS |
| 1 s transactions + 32 MiB cache | 13.7 s | 1572 MiB | 77.5 MiB | rejected: memory |
| 1 s transactions + `wal_autocheckpoint=10000` | 18.3 s | 2044 MiB | 26.1 MiB | rejected: more writes |
| 2 s transactions | 17.4 s | 1548 MiB | 26.3 MiB | rejected: −7 % writes for 2× event delay during scans |

Cause: small transactions dirty pages scattered across the B-tree indexes
(`key` and `(parent, name)` are in effectively random order relative to the
scan). Every commit and checkpoint rewrites them. Larger transactions
amortize this.

Two other effects were found and fixed with measurements:

- **Benchmark interference.** Polling `Status` (a `COUNT` over the index)
  every 10 ms kept a read snapshot open almost continuously. That blocked
  WAL checkpoints, which doubled the writes of the 50 % run (3.47 GiB) and
  left a 273 MiB WAL behind. With 250 ms polling after the first 2 s, and a
  `TRUNCATE` checkpoint at the end of a pass, the figures were 1.70 GiB and
  a 0 B WAL.
- **Idle heap retention.** Allocations from short-lived query threads and
  zbus remained as freed-but-resident glibc arena memory: 15.4 MiB idle RSS
  after a restart at 1M. Capping arenas at 2, plus `malloc_trim` after
  KRunner `Teardown` and `Status`, plus `PRAGMA shrink_memory` when idle,
  brought it to 9.2 MiB.

The index backend itself was chosen with a separate prototype
(`prototype/backend-bench/README.md`): SQLite FTS5 vs Tantivy vs FST vs a
linear scan, on 1M names.

## 5. Live desktop verification (not a benchmark)

On the Plasma 6.8.2 session (2026-10-31), I installed with
`scripts/install.sh --config <test config>`, indexed a test directory in
`/tmp`, and uninstalled with `--purge` afterwards:

- KRunner loaded `ferroseek.desktop` and called `Match`. The first attempt
  failed with "name is not activatable" until the session bus
  (dbus-broker) was told to `ReloadConfig`, which the install script now
  does.
- KRunner's first `Match` started `ferroseek.service` through D-Bus
  activation. The result rendered in KRunner under "Images" with the MIME
  icon and the parent folder as subtext (checked by screenshot).
- The default action, called over D-Bus exactly as KRunner does (no input
  injection tool was available to press Enter), opened the image in
  Gwenview. Gwenview ran in its own `app-org.kde.gwenview@….service`, not in
  `ferroseek.service`.
- "Open Containing Folder" opened Dolphin with the file selected.
- A file created, then renamed, appeared in KRunner `Match` results under
  its new name, and the old name returned 0 results.
- Idle memory of the D-Bus-activated daemon (tiny index): 7.8 MiB VmRSS
  (1.4 MiB anonymous), cgroup `MemoryCurrent` 2.0 MiB.

## 6. Baloo comparison protocol (not yet executed)

It requires the owner's explicit permission to enable Baloo temporarily.
The aim is equivalent features: names only, the same root, warm cache, and
the same hardware.

1. Back up `~/.config/baloofilerc`. Configure Baloo with
   `folders=<corpus root>`, `only basic indexing=true` (no content) and the
   same hidden-file setting. Note that Baloo's `exclude filters` cannot match
   FerroSeek's exactly, so list the differences.
2. Use the same `ferroseek-bench` corpus (same seed). Run `balooctl6 purge`
   on that dedicated config, then `balooctl6 enable` and time until
   `balooctl6 status` reports idle. Sample `baloo_file` and
   `baloo_file_extractor` from `/proc/<pid>/{status,io}` the same way.
3. Query latency: run the same query list through `baloosearch6 -l 15`
   (process per query, which disadvantages Baloo) and also through the
   `org.kde.runners.baloo` D-Bus `Match` (equivalent to FerroSeek's D-Bus
   numbers).
4. Incremental: the same create/rename/delete loop, observed through
   `org.kde.runners.baloo` `Match`.
5. Restore `baloofilerc`, `balooctl6 disable`, and delete the test index.

Differences that already prevent an apples-to-apples comparison:
- Baloo matches word prefixes only (bug 333037); FerroSeek matches
  substrings.
- Baloo's runner issues ten type-filtered queries per keystroke.
- Baloo indexes extended attributes and tags.

## 7. Limitations

- **Warm page cache only.** Dropping caches (`/proc/sys/vm/drop_caches`)
  needs root, which this project does not use. Cold-cache numbers will be
  slower, mainly for the first queries and the restart reconcile.
- **Synthetic corpus.** Real home directories have different name
  distributions and directory shapes. Re-run on your own data with
  `ferroseek index` plus `ferroseek status`.
- **Single run per size.** There are no repetitions or confidence
  intervals. Latency percentiles come from 390 queries; change → visible
  figures come from 30 iterations, so p99 is effectively the maximum.
  Occasional outliers up to ~0.7 s were seen for rename visibility at 1M;
  the cause has not been investigated yet.
- **Disk-write figures are block-layer bytes**, not device writes after
  Btrfs compression.

## 8. After code-review fixes: 2026-10-31

**Verdict:** ordinary filename queries remain responsive in this run, but
path-only scans are a clear latency concern at 1M files: about half a second
per KRunner request. These are new absolute measurements of `ce6f6ee`,
**not a controlled before/after speed comparison**. Earlier sections retain
their original measurements and limitations.

Full reports, binary hashes, reproduction commands and caveats are in
[`benchmarks/review-2026-10-31/`](benchmarks/review-2026-10-31/README.md).
Same machine, release profile, warm cache, seed 42, private D-Bus session;
new Btrfs corpus directories. No Baloo or KDE settings were changed.

| Query case | 100k generated files | 1M generated files |
|---|---:|---:|
| Ordinary mix, D-Bus p50 / p95 (390 queries) | 0.77 / 3.08 ms | 2.53 / 24.81 ms |
| Exact sentinel name, D-Bus | 1.180 ms (1.175–1.400) | 4.591 ms (4.587–4.624) |
| Path only, D-Bus | 50.997 ms (50.639–51.480) | 524.328 ms (521.475–528.481) |
| Path + name, D-Bus | 8.120 ms (7.943–8.215) | 109.594 ms (108.742–110.100) |
| `within` + name, library | 7.618 ms (7.545–8.058) | 113.055 ms (112.787–114.417) |
| Missing path, D-Bus | 50.748 ms (50.506–51.267) | 527.119 ms (525.942–543.688) |
| Slash-only `//`, D-Bus | 0.071 ms (0.068–0.079) | 0.071 ms (0.070–0.078) |

Except the ordinary mix, values are **medians of five round medians**,
with their min–max range in parentheses; three requests per round, 15
measured requests per case/transport. All 330 measured responses across the
two sizes were verified against explicit expected results, with zero
errors or mismatches. This is not a concurrency or cold-cache measurement.

Why path queries cost more: EXPLAIN confirms a full `entries` scan for
path-only queries. Library CPU time is approximately equal to wall time
on the slow cases, pointing to CPU work in scanning/filtering/path resolution.
The bounded result set no longer truncates candidates *before* filtering;
a selective filter may therefore inspect the whole candidate source.
The similar library and D-Bus timings show that transport does not explain
the hundreds of milliseconds. Exact attribution within SQLite decoding,
allocation and path construction still needs profiling.

The legacy relevance percentages in the raw reports are **not used as
correctness evidence**: that older metric treats skipped hidden/non-UTF-8
samples as successes. The new explicit result checks avoid this issue.

Lifecycle measurements (one run per size; live-update percentiles use 30
operations per kind):

| Metric | 100k generated files | 1M generated files |
|---|---:|---:|
| Indexed files / watched directories | 95,200 / 5,100 | 953,595 / 51,000 |
| Initial indexing, 100% duty setting | 1.15 s | 19.01 s |
| Database size | 25.1 MiB | 251.7 MiB |
| Idle RSS after initial scan | 9.0 MiB | 15.7 MiB |
| Idle RSS after restart | 7.3 MiB | 9.4 MiB |
| Idle CPU / context switches, 30-second observation | 0.000% / 0 per second | 0.000% / 0 per second |
| Create → visible in Match, p50 / p95 | 202.49 / 208.20 ms | 203.17 / 213.46 ms |
| Rename → visible in Match, p50 / p95 | 202.31 / 204.88 ms | 203.55 / 204.51 ms |
| Delete → absent from Match, p50 / p95 | 202.16 / 206.51 ms | 202.19 / 203.58 ms |
| Burst: 10k creates, convergence after last write | 0.18 s | 0.19 s |
| Burst: 10k renames, convergence after last rename | 0.35 s | 0.61 s |
| Inotify overflows in burst test | 0 | 0 |
| Restart → idle | 0.17 s | 1.03 s |
| SIGKILL recovery: indexed / expected files | 95,200 / 95,200 | 953,595 / 953,595 |

The 200 ms configured debounce dominates isolated update visibility. All
bursts converged and both recovery checks matched expected file counts;
both daemon logs were empty. CPU/context-switch zeros describe the observed
window and clock resolution, not a guarantee of zero work indefinitely.
RSS readings from `/proc` are approximate: the raw 1M run even sampled
VmRSS above the later VmHWM, so those values cannot establish a precise
peak-memory bound. No new claim about improvement over the old version is
made from these single lifecycle runs.

## 9. Path optimization: 2026-10-31

The expensive **selective** path scans are substantially improved, using
existing directory-name FTS and parent indexes. No persistent index or schema
was added. Baseline `6f7172d` and accepted candidate `a4c33c8` were measured
on the same machine with preserved release binaries. Complete reports,
hashes, scripts and caveats are in
[path-optimization evidence](benchmarks/path-optimization-2026-10-31/README.md).
Earlier sections describe earlier runs, not the current result.

### Repeated warm queries

Five alternating baseline/candidate daemon lifetimes per dataset, same
corpus/index, five rounds × three requests per selective case per process.
Figures below are medians of the five process estimates, in milliseconds.
Only D-Bus timings are comparable across binaries; the harness's library
rows always use its own candidate library.

| Generated files | Path-only before → after | Path + name before → after | Missing path before → after |
|---|---:|---:|---:|
| 10,000 | 4.886 → 0.487 | 0.634 → 0.529 | 4.861 → 0.360 |
| 100,000 | 52.315 → 0.861 | 8.163 → 0.899 | 51.820 → 0.519 |
| 1,000,000 | 529.820 → 2.032 | 109.939 → 2.069 | 531.151 → 0.589 |

At 1M, path-only process estimates ranged 528.298–534.423 ms before and
1.913–2.100 ms after; path+name 109.224–111.362 vs 1.934–2.204 ms. These
support roughly 261× and 53× improvements for these fixtures, not all path
queries. The small 10k path+name ranges overlap. Exact filename latency
was essentially unchanged: 4.524 → 4.564 ms. All 4,950 measured explicit
selective-case responses matched their expected paths/counts or emptiness.

| 1M query workload | Before p50 / p95 / p99 | After p50 / p95 / p99 |
|---|---:|---:|
| Common words (350 requests/process, five processes) | 3.61 / 4.22 / 4.58 ms | 3.49 / 3.95 / 4.39 ms |
| Broad `corpus/` (15/process, five processes) | 3.33 / 3.35 / 3.36 ms | 3.46 / 3.65 / 3.66 ms |
| Broad `corpus/ report` (15/process, five processes) | 12.41 / 13.14 / 13.39 ms | 9.19 / 9.79 / 10.40 ms |
| Original mixed 390-query workload (one lifecycle/process per side) | 2.57 / 25.07 / 27.54 ms | 2.48 / 24.80 / 27.48 ms |

Repeated rows are medians of per-process percentiles, not pooled
percentiles. Fifteen-sample p99 is effectively a maximum. The common-word
subset must not be substituted for the original mixed workload: the mixed
p95 target below 10 ms **is not met**. Broad path-only probing adds roughly
0.13 ms here. Short directory components, broad anchors and unselective
filters can still use scanning fallbacks.

### Indexing, resources and updates

Full lifecycle suites ran once per side at each of 10k, 100k and 1M; all six
raw reports are retained. The 1M comparison below is descriptive, not a
statistically established indexing/resource speedup.

| 1M metric | Before | After |
|---|---:|---:|
| Initial scan, 100% duty: wall / CPU | 20.17 / 23.75 s | 18.68 / 22.64 s |
| Initial scan, 100% duty: VmHWM | 21.0 MiB | 23.5 MiB |
| Initial scan, 100% duty: read / written | 0 / 1980.5 MiB | 21.9 / 1602.9 MiB |
| Database after initial scan | 250.5 MiB | 251.2 MiB |
| Idle RSS after initial scan / restart | 11.6 / 9.7 MiB | 15.6 / 10.1 MiB |
| Restart → idle | 1.03 s | 1.20 s |
| Burst VmHWM | 26.3 MiB | 25.8 MiB |
| Burst disk read / written | 0 / 50.1 MiB | 1.7 / 38.7 MiB |
| Mid-index SIGKILL recovery → complete | 14.59 s | 13.81 s |

Both indexed 953,595 files and watched 51,000 directories; hidden/excluded
files explain the difference from the generated count. Recovery started
from partial commits (341,651 / 360,363 files) and reached the independent
expected final count in both builds. At 10k the kill arrived after indexing
completed, so that size does not demonstrate mid-index recovery.

The candidate's create/rename/delete visibility medians were 203.21 /
203.34 / 202.48 ms, consistent with the configured 200 ms debounce. A 10k
create burst converged in 0.19 s after the last write, renames in 0.51 s,
a directory rename in 225 ms, and tree deletion in 0.37 s. No inotify
overflow occurred; these runs do not exercise overflow recovery by force.
The 30-second idle windows recorded zero CPU ticks and context switches,
with four threads. After a Match/Teardown cycle and worker expiry, separate
smaps readings were 10,300 / 10,428 KiB RSS, four threads each.

The candidate has no extra on-disk data structure. Small SQLite file-size
differences and variable write volumes do not establish added index cost.
Peak memory remains higher than idle memory; the initial idle RSS increase
in this single pair is reported rather than hidden. Per-directory listing
memory and one inotify watch per directory still scale with workload.

A single default-throttle lifecycle pass wrote 1714.2 MiB before and
3132.4 MiB after (38.10 / 42.17 s). To investigate that apparent regression,
five alternating fresh indexes per build were created on the **same exact**
1M corpus at 50% duty. The write spike did not recur:

| Repeated default-throttle indexing | Before median (range) | After median (range) |
|---|---:|---:|
| Wall time | 36.70 (36.44–37.69) s | 36.25 (36.19–38.01) s |
| CPU time | 26.90 (26.57–27.13) s | 26.46 (26.36–27.27) s |
| Disk written | 1772.2 (1679.4–1820.9) MiB | 1707.7 (1602.2–1867.2) MiB |
| Peak RSS | 21.8 (21.2–24.5) MiB | 23.5 (23.3–23.8) MiB |
| Database + WAL | 251.1 (250.7–252.8) MiB | 251.2 (251.1–252.3) MiB |

These overlapping ranges show no demonstrated systematic indexing/write
regression. They do not establish the cause of the earlier spike or a
statistically significant indexing improvement. Raw repeated reports are
in the evidence directory's `initial50/`. Full lifecycle tests were not
repeated five times: repeated queries cover all three sizes, indexing
repetitions focus on the 1M cost concern.

### Real filesystem and cache effects

Five independent indexes per version of `/usr/include` (40,883 entries)
were verified against an independent raw-path `os.walk` oracle. Every CLI
query retrieved all matches; zero set mismatches or process errors occurred.
Median warm CLI times: `linux/` 20.23 → 2.65 ms (839 matches), `bits/`
19.82 → 2.40 ms (496), and a missing directory 19.36 → 1.54 ms. Broad
`include/stdio` stayed 19.53 → 19.17 ms (two matches), demonstrating the
remaining fallback cost. Both databases occupied exactly 8,245,248 bytes.

Five alternating missing-path CLI queries after advisory database-page
eviction measured median wall time 1416.88 → 31.10 ms. These include process
startup, unlike D-Bus measurements. `ru_inblock` was positive for every
sample (baseline roughly 424,800 input operations, candidate 2,736), so
filesystem I/O occurred. `posix_fadvise(DONTNEED)` is advisory: filesystem
metadata and device caches were not reset, and this is **not a whole-machine
cold-cache benchmark**. No privileged cache drops were used.

### Correctness and release scope

Independent path-list tests cover nesting, moves/deletions, Unicode and
non-UTF8 names, symlinks, hidden/excluded entries, prefixes/substrings,
missing paths, large match sets and concurrent rename snapshots. Reader
lifecycle tests cover Teardown races and database replacement. Actual
Plasma activation, results, file/folder opening, live changes, indexing and
restart were retested; see [release readiness](RELEASE_READINESS.md).
Baloo remained disabled. No fair Baloo comparison has been performed.

## 10. MCP server: 2026-10-31

These numbers measure `ferroseek-mcp` (docs/MCP.md) on its own, over its
real stdio JSON-RPC protocol, with `scripts/benchmark-mcp.py`.

- The server opens the generated 100k and 1M indexes read-only. The allowed
  directory is the corpus root, which contains the index root, so the fast
  unscoped path is used.
- The query set is 300 deterministic 3–8 character substrings of real
  indexed names plus 30 no-hit queries, with 20 warm-up calls first.
- Latency is the client-side round trip, including JSON encoding and the
  `lstat` verification of each returned result.
- Memory and CPU come from `/proc/<pid>`.

Raw reports and hashes are in
[mcp-2026-10-31](benchmarks/mcp-2026-10-31/README.md).

| metric | 100k | 1M |
|---|---|---|
| startup: spawn → `initialize` reply | 1.2 ms | 1.2 ms |
| `search_files` limit 20, p50 / p95 / p99 / max | 1.30 / 9.26 / 9.97 / 10.92 ms | 4.81 / 12.08 / 15.74 / 71.67 ms |
| `search_files` with `root` = subdirectory, p50 / p99 | 3.92 / 16.83 ms | 9.12 / 42.34 ms |
| `get_file_info` p50 / p99 | 0.09 / 0.16 ms | 0.09 / 0.17 ms |
| `index_status` p50 (default: no whole-index totals) | 0.07 ms | 0.06 ms |
| `search_files` JSON-RPC line, p50 / max | 6.2 / 10.6 KiB | 6.2 / 8.9 KiB |
| RSS after `initialize` (anonymous) | 5.0 MiB (0.4) | 5.1 MiB (0.4) |
| RSS / VmHWM after 650 requests | 12.9 / 13.2 MiB | 15.3 / 15.9 MiB |
| idle RSS 12 s later (anonymous) | 10.4 MiB (3.8) | 11.4 MiB (4.8) |
| idle CPU, context switches (30 s) | 0.000 %, 0.00/s | 0.000 %, 0.00/s |

These are final numbers, after all three review rounds. Every measured build
returned the same results (5309 for 330 queries at 1M). The 1M median of
`search_files` evolved as follows:

| build | p50 | change |
|---|---|---|
| first version | 12.66 ms | `within` always set |
| unscoped fast path | 2.89 ms | |
| race-safe `openat` metadata walk | 3.22 ms | |
| policy inside the index budget, first cut | 7.04 ms | |
| memoized parent verdicts + pre-classified deny patterns | 6.37 ms | |
| stage 2 reuses the candidate stage's directory cache + head truncation for `accept`-only searches | 5.69 ms | a ranking bug, found in review round 3 |
| truncation replaced by an exact top-k early stop (final) | 4.81 ms | |

The starvation fix inherently resolves every candidate's parent directory;
that is the remaining cost (about 1.6 ms) compared with 3.22 ms. `index_status` went from
135 ms to 0.07 ms because whole-index totals are now opt-in.

The stage-2 cache reuse also runs for the daemon's own searches. The
daemon benchmark on a fresh 100k corpus showed no regression: KRunner
`Match` p50 0.66 / p99 3.36 ms against 0.77 / 3.30 ms in the §8 run, with
relevance at 100 %. The raw report is `daemon-regression-check-100k.md` in
the evidence directory.

Two changes during development were measured on the same 1M index with the
same query set:

| | before | after |
|---|---|---|
| **Unscoped search** when the allowed directory contains every index root (previously always `within`, which seeds a subtree walk) | p50 12.66 ms, p99 91.95 ms | p50 2.89 ms, p99 10.11 ms |
| **Idle release** (drop the cached reader + `malloc_trim` 5 s after the last request; glibc arenas capped at 2) | idle anonymous memory 15.1 MiB | 3.2–5.3 MiB |

Result counts were identical before and after (5309 results for 330
queries).

A first version of the idle release woke up about 0.1 times per second
because of a stored `Notify` permit. It was fixed to 0.00/s.

**Limitations.**
- Each size was run once, with a warm cache and synthetic names.
- `root` filters on subdirectories and `index_status` (a full COUNT) are
  the slow cases.
- When not running, the server costs nothing: it is a stdio child of the
  agent and exits on EOF, as the protocol tests verify.

## 11. MCP validation against filename tools (2026-10-31)

[Full method, tables, raw evidence and reproduction commands](benchmarks/mcp-validation-2026-10-31/README.md).
Five process repetitions compared resident MCP round trips with fd, rg --files
and a private plocate index on four generated repositories plus a public source
snapshot. All 700 steady-state responses matched an independent filesystem
oracle. Results depend on query, scope and invocation pattern; there is no
universal winner. Index construction, watcher freshness, CPU, I/O and memory
are reported separately, including measurement limitations.

A regression with 8,300 deleted higher-ranked entries reproduced zero recall
for two survivors. Recovery now filters vanished paths inside the candidate
budget: all survivors returned in five paired trials, without a measurable
ordinary-query latency change. The million-file scoped p95 remains around
89 ms in this workload. An unconvincing scoped-ranking optimization was
rejected. This work did not modify the live desktop service or its soak test.

## MCP access-mode regression check

See [the isolated access-mode comparison](benchmarks/mcp-access-modes/README.md)
for five process repetitions on 20,000 synthetic files, independent result
oracles, stdio round trips, and resource measurements. This is a selective
warm-cache regression check, not a new million-file or desktop benchmark.

## 12. Launcher interaction latency

The historical November 2 snapshots below predate the WGPU material path. New launcher runs
include graphics adapter initialization; the historical software results do not
characterize its startup cost. Per-run diagnostics remain in the graphical CI
artifacts, with physical display presentation still excluded.

The isolated graphical harness measures the release launcher and real daemon
together. It generates 10,007 synthetic files, waits for indexing to finish,
and starts virtual KWin with a software renderer, a temporary HOME/XDG tree and
a private bus with no activation services. Launcher processes use `--private`,
so history and favorites are disabled. Current launcher/daemon application data
contains synthetic entries only; KWin and Spectacle separately read fixed system
resources. Historical runs below identify their earlier runner-application workload. A separate test window establishes
keyboard input before the launcher and receives input again after it closes.
Test-only Wayland fake input types `report` into the real search widget, edits
the action filter and returns to the main field. No UI messages are injected.

```sh
cargo build --release --locked -p ferroseek-launcher -p ferroseek-daemon
export XDG_RUNTIME_DIR=$(mktemp -d)
LIBGL_ALWAYS_SOFTWARE=1 FERROSEEK_SMOKE_INTERACTION=1 FERROSEEK_SMOKE_MODE=all \
  xvfb-run -a tests/launcher-smoke.sh target/release target/launcher-interaction
```

The workflow `Launcher interaction` installs the Linux test dependencies and
retains PNGs, per-process diagnostic logs and `latency.json`. The JSON records
the OS, CPU, KWin version, keyboard profile, observed logical output geometry/scale
and every sample. Schema 3 records the matching query generation,
`query_ready_us`, `ready_to_frame_us`, `keyboard_focus_us`, observed renderer and
live RSS/HWM snapshots. There are 20 process launches per mode by
default (`FERROSEEK_SMOKE_TRIALS=2…100`); the first of each mode/renderer group
is reported separately and the remaining 19 use nearest-rank p50/p95 and maximum. Neither set claims cold
caches: no caches are dropped and the daemon remains warm between launches.

- **First redraw:** entry into launcher `main` to its first redraw event;
  excludes process creation before `main`, and may precede initial results.
- **Query redraw:** receipt of the final typed-character edit to a redraw
  event after current results are applied. Includes application matching,
  backend queueing, D-Bus/search, result processing and the redraw callback;
  excludes injected-key dispatch before receipt and compositor presentation.
- **Keyboard focus:** entry into launcher `main` to the activation thread's first
  compositor keyboard-focus notification. It is not widget readiness or a physical
  shortcut-to-display measurement. Real typed input checks follow that notification.
- **RSS/HWM snapshot:** the live launcher's `/proc/<pid>/status` `VmRSS` and
  `VmHWM`, read after the harness observes current results and before further
  input. Values include shared pages and exclude GPU memory. HWM covers the
  process only through that read; polling can delay the snapshot after the event.
- Superseded or hidden queries, failures and stale results do not count as successful
  query samples. Pending redraw callbacks older than result application are
  excluded. Logging and callbacks introduce measurement overhead.

This workload is a repeatable diagnostic baseline, not a desktop speed claim
or a CI speed threshold. Current separate functional checks include activation
context, existing synthetic targets, the named layouts and fractional-scale profile;
these are not retroactive claims about the first recorded run below. Physical
displays, multiple monitors, IMEs, other toolkits and real desktop associations
remain outside the measured setup. The signal is `RedrawRequested`, before drawing
completes; it does not measure completed rendering or monitor presentation.

Current samples split the final edit into **results applied** (`query_ready_us`)
and **results applied → redraw event** (`ready_to_frame_us`). Both come from the
same successful generation. The second interval is the difference of query-relative
microsecond durations; it includes scheduling until the redraw event, not rendering.
Do not add independently sorted stage percentiles to infer a total percentile.

`FERROSEEK_SMOKE_MODE` selects `auto` (default), `opaque`, `software` or `all`.
The first two use the normal renderer selection; `software` forces tiny-skia.
Each process clears inherited `ICED_BACKEND` before applying its mode. `all`
rotates the order on each repetition, for 60 measured opens at the default count,
sharing one machine, compositor and warm daemon. The three desktop-profile CI jobs
use `all`; profile jobs can still run on different hardware.

Every sample records the renderer actually encountered in the root widget's
draw path. `summaries` groups by requested mode and observed renderer, for example
`auto/wgpu` or `software/tiny_skia`; each group has `first_trial` and `repeats`.
Repeat metrics have `n`, nearest-rank `p50`, `p95` and `max`, with units in their
field names (`_us` or `_kib`). An empty repeat group has null metrics. The
controlled graphical check requires its expected WGPU/tiny-skia groups; it
does not set a latency threshold or silently mix fallback renderers.

Reports include the launcher's SHA-256 and requested graphics settings. Renderer
events distinguish WGPU from tiny-skia, not Vulkan from GL or a software adapter
from a hardware GPU. Per-process logs remain the source for timestamp validation.
Material pixel checks use separate processes and prove blur only for those
controls. After the measured opens, `latency.json` retains their memory snapshots
with `validation: incomplete`. It changes to `validation: passed` only after all
functional/material checks pass; the verifier rejects incomplete reports. Missing
interaction prerequisites or non-OpenGL compositing are inconclusive
failures, rather than successful skips. Nothing here certifies physical displays.
Preview controls wait for an observed `surface_resized` width after opening or
closing the sidebar. Content readiness alone can precede the resized surface;
the pixel assertions still decide whether the resulting image is correct.
Each measured process also appends its completed sample to `samples.jsonl` before
further input. A failure during a later trial retains those earlier live-memory
readings, even when the full report cannot be constructed. The stream is not a
validation pass; the verifier reconciles it with a completed report and raw logs.

Check a completed report against the original process logs and its executable:

```sh
python3 scripts/verify-launcher-report.py target/launcher-interaction \
  --binary target/release/ferroseek-launcher
```

The verifier checks sample counts, focus/backend/generation evidence, same-query
intervals and every aggregate. The graphical workflow runs it before the separate
screenshot-only check. Reports and raw logs must be kept together to rerun it.
Use a new or empty interaction output directory for every run. The harness refuses
existing contents before starting its bus or services, preserving previous evidence
and excluding stale logs or readiness markers from a new measurement.

### First recorded run — 2026-11-02

[Source and complete run](https://github.com/Lucenx9/ferroseek/actions/runs/41495909474):
commit `14652bb`, GitHub Ubuntu 24.04 runner, 4 vCPUs (AMD EPYC 9V45),
Linux 7.0.0-1017-azure, glibc 2.39, KWin 5.27.11 with llvmpipe, rustc 1.99.1,
release profile with thin LTO and one daemon throughout the 20 opens.
This environment differs from sections 1–11.

| Diagnostic signal | First process (ms) | Subsequent p50 (ms, n=19) | Subsequent p95/max (ms, n=19) |
|---|---:|---:|---:|
| `main` → first redraw event | 263.89 | 142.70 | 165.85 |
| Final character edit → results redraw event | 8.30 | 17.47 | 31.07 |

[Raw samples and environment](benchmarks/launcher-interaction-2026-11-02.json)
retain the exact microsecond values. Independently checked every sample and
percentile against the 20 process logs. All opens verified 15 actual daemon
file results, editing after asynchronous results, and background input after
closing. The first also verified filter clearing, menu closure and main-field
editing after those transitions. Inspected the three original screenshots.
No startup optimization or cross-machine speed comparison is established.


## 13. Launcher desktop profiles

[Recorded run](https://github.com/Lucenx9/ferroseek/actions/runs/41529913635),
commit `373b6c0952f43be5582a9edb20a98da962025d6b`, verified US 1×, French AZERTY
1× and US 1.5× in virtual KWin 5.27.11 on Ubuntu 24.04. Each independent job used
one observed 1280×800 logical output and the full keyboard/pointer activation
checks. This is a virtual validation matrix, not physical Plasma 6 certification.

All jobs report Linux 7.0.0-1017-azure, glibc 2.39 and a software renderer. US 1×
used an AMD EPYC 9V74; the other two used EPYC 7763. The daemon was warm, the corpus
contained 10,007 synthetic files, and each profile retained 20 numeric samples.
The first process is separate from 19 repeats. Values are milliseconds:

| Profile | First redraw, first process | First redraw p50 | Query ready p50 | Ready → redraw p50 | Total query p50 | Total query p95/max |
|---|---:|---:|---:|---:|---:|---:|
| US 1× | 844.325 | 215.921 | 11.315 | 7.867 | 21.291 | 46.396 |
| AZERTY 1× | 1117.565 | 248.173 | 14.792 | 2.828 | 24.708 | 49.074 |
| US 1.5× | 415.877 | 245.052 | 12.359 | 4.889 | 17.186 | 44.415 |

[Environment, all samples and derivation](benchmarks/launcher-desktop-2026-11-02.json)
explicitly label their derivation from this run's schema 1 reports and matching
raw `query_ready`/`query_frame` events. Current schema 3 exports
those intervals per sample and groups summaries by mode/renderer. The recorded jobs predate full application-data isolation
in the current harness; they included runner-installed entries. This and differing
hardware prevent treating the table as a layout comparison or an optimization gain.
For n=19, nearest-rank p95 equals maximum. Per-stage percentiles are independent.

To reproduce the current workload, use the command in section 12 with
`FERROSEEK_SMOKE_PROFILE=us-1x`, `fr-1x` or `us-1.5x`; preserve each profile in its
own output directory. The workflow runs all three. It checks actual KWin geometry,
scale, ordinary key symbols and native screenshot sizes before assigning evidence
to a profile. Preset queries wait for compositor keyboard focus; action text waits
for confirmed widget focus. Typeahead before those transitions complete is not
established by this suite. Raw support information, timings and original PNGs are
retained as artifacts. No presentation or physical-desktop latency is measured.
