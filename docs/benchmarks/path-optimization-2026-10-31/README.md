# Path optimization evidence — 2026-10-31

Production baseline: `6f7172d` / daemon SHA-256
`b202d843d527512bf89629ceb8a7e406bf61e65fed5485fa4ba919a15546869a`.
Candidate: `a4c33c8` / daemon SHA-256
`15a4b10240c83510842cf51a9f10259074d702432ceff81e0bb2ab4d08614d67`.
The benchmark extensions are committed alongside this report. Later harness
argument safety checks do not affect timed work. The baseline binary was
preserved before editing; both use Rust 1.94 release/thin-LTO on the same
Ryzen 5800X, Btrfs/NVMe, Linux 7.3.0-4-cachyos machine.

## Comparable query measurements

`pairs/` contains five alternating baseline/candidate daemon processes per
size, at 10k, 100k and 1M generated files. Each process performs an untimed
warmup, then five rounds of three requests for each selective query case.
The fixture is created, indexed, verified, removed and disappearance checked
in every process. Reports additionally include 350 common-word requests and
15 requests each for broad root/path cases. The service runs on a private
bus; configuration, corpus and index are benchmark-only.

Compare **D-Bus rows only** across sides: the same current harness uses its
own candidate library for library rows even when driving the old daemon.
This deliberate limitation prevents labelling candidate library timings as
baseline results. `within+name` has no D-Bus Match equivalent, so its library
row is candidate-only. `paired-summary.json` aggregates the five independent
processes; median p50/p95/p99 values are medians of per-process percentiles,
not pooled percentiles. The selective-query main figure is the median of
per-process round-median estimates. Raw ranges and sample maxima are retained.
A p99 from 15 samples is essentially the maximum, not a stable tail estimate.

All 30 processes completed without query errors or mismatches. Each checked
165 selective-case responses, with explicit expected paths/counts or empty
results. Common-word and broad cases check nonempty responses but are not
exhaustive relevance oracles. Unit/integration tests and the real-filesystem
workload provide separate correctness coverage.

Warm filesystem cache is intentional: both versions share the same corpus
and index, and each warms its connection before timing. New processes give
independent reader/allocator lifetimes, not independent cold disks. No global
cache dropping, Baloo comparison, or performance extrapolation was performed.
The desktop remained active (typical load average around 1 on 16 logical
CPUs); this is not an isolated laboratory host. Earlier rejected prototypes
and their pilot measurements are excluded from accepted results.

Reproduction:

```sh
# Save release binaries from baseline 6f7172d before building the candidate.
# Populate synthetic benchmark work directories with the normal bench runner.
# datasets.json maps counts to their existing work dirs, for example:
# {"10000":"/absolute/bench-10k","100000":"/absolute/bench-100k",
#  "1000000":"/absolute/bench-1m"}
python3 scripts/benchmark-pairs.py \
  --baseline /absolute/baseline-bin --candidate /absolute/candidate-bin \
  --harness /absolute/candidate-bin/ferroseek-bench \
  --datasets datasets.json --output /absolute/new-results --repetitions 5
```

## Real filesystem oracle

`real.json` measures `/usr/include` without changing its files: 40,883
entries, including directories and symlinks. Five independent indexes per
version are created in temporary private data directories, alternating
versions. Every query retrieves all matches and compares the exact raw path
set to a separate `os.walk` oracle, with case-insensitive path-substring
matching. This measures CLI startup + connection + search + output, unlike
resident D-Bus timings. Each case has an excluded first warmup plus five
samples. There were zero set mismatches. Both versions produced an 8,245,248
byte index; median indexing time was 0.329 / 0.327 seconds (baseline / candidate).

```sh
python3 scripts/benchmark-real.py --baseline /absolute/baseline-bin \
  --candidate /absolute/candidate-bin --output /absolute/real.json
```

## Profiling and rejected approaches

The original path-only plan was `SCAN entries`, decoding about 1,004,595
rows in the 1M corpus. Thread CPU/wall near 1 supported a CPU-bound scan/path
construction cost. We reused existing FTS directory-component matches and
`entries_parent_name` traversal instead of adding materialized full paths
or another indexing backend. That avoids new persistent structures and
subtree-rename write amplification; no claim is made that an unimplemented
alternative was empirically slower.

Two prototype regressions were found and fixed before acceptance:

- Unbounded directory expansion made `corpus/` roughly one second. Root
  anchors and scopes exceeding 256 seed/descendant directories now fall
  back to the original streaming strategy. Small scopes (at most 4096
  entries) scan directly instead of traversing global filename postings.
- Ordering FTS candidates by `e.id` caused `USE TEMP B-TREE FOR ORDER BY`.
  Ordering by `names.rowid` produces `SCAN names VIRTUAL TABLE INDEX 64:M1`
  with no temporary sort and preserves the same row-ID ordering. Prefix
  candidates explicitly retain key/id order, preventing a false ranking
  result caused by SQLite switching to parent-index order.

Full-path verification remains mandatory after scope selection. Queries
with only short directory components, broad anchors and unselective name
filters can still scan extensively. Existing candidate-budget ranking is
approximate for very large match sets; this optimization does not promise
exhaustive global top-k ranking. No allocation-level flamegraph attribution
is claimed: perf/strace were not installed; query plans, row counts and CPU
measurements identify the dominant scan/planner costs.

## Lifecycle, repeated indexing, advisory cache eviction and session idle

`lifecycle/` contains one full suite per version per size. Each includes
indexing at 50/100% duty, CPU/IO/RSS, 390 mixed queries, live/burst updates,
restart and SIGKILL recovery. Use fresh nonempty-safe work directories:

```sh
/absolute/harness/ferroseek-bench --files 1000000 --seed 42 \
  --work /absolute/new-work --bin-dir /absolute/version-bin
```

`initial50/` repeats initial indexing five times per version, alternating
baseline then candidate on the exact same 1M corpus. The earlier single
candidate write spike did not recur. Each invocation removes **only the
benchmark work/data index** and rebuilds it; never point this at user data.

```sh
# Repeat five times per version, alternating versions; save stdout separately.
/absolute/harness/ferroseek-bench --files 1000000 --seed 42 \
  --work /absolute/existing-benchmark-work --bin-dir /absolute/version-bin \
  --reuse-corpus --only-initial --cpu 50
```

`cache.json` contains five alternating missing-path CLI requests with
advisory eviction of the benchmark database using posix_fadvise. It records
wall/CPU time and input block operations; it does not reset all machine or
device caches. `idle-*.json` records private-bus daemon memory and threads
after Match, Teardown and twelve seconds of worker expiry. Reproduce with:

```sh
python3 scripts/benchmark-cache.py --help
python3 scripts/benchmark-session-idle.py --help
```

These scripts expose explicit binary/work/output paths; preserve each
version's output separately. `/proc/pid/io` writes are process-attributed
block-layer bytes, not physical NVMe writes after filesystem compression.
RSS/HWM from `/proc/status` is approximate; smaps session snapshots provide
a separate idle check. The path optimization adds no persisted structure.
