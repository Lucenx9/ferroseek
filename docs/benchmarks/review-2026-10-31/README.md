# Review remeasurement — 2026-10-31

Production code: `ce6f6ee`. Harness: the `review.rs` addition committed with
these reports. Both sizes used the same release binaries and seed 42.

- `ferroseekd` SHA-256: `b202d843d527512bf89629ceb8a7e406bf61e65fed5485fa4ba919a15546869a`
- `ferroseek-bench` SHA-256: `b58d5c7ed2435d7791b3589745bc22f069ec76bd03346635575690e73ce8abae`
- Linux `7.3.0-4-cachyos`, rustc `1.94.0`, 16 logical CPUs.
- Same Ryzen/Btrfs/NVMe environment as BENCHMARKS.md §1; new directories on
  `/home`, not tmpfs. Initial load average: `1.28, 1.13, 0.94`.
- Normal desktop/browser/T3 processes remained active. This was not a
  dedicated benchmark machine; a short workspace test run also overlapped
  the 1M initial-index stage. No claimed before/after speedup.

```sh
cargo build --release --locked
# Use a fresh parent: the existing harness deletes its --work directory.
bench_root=$(mktemp -d "$HOME/.cache/ferroseek-review-bench-XXXXXXXX")
target/release/ferroseek-bench --files 100000 --work "$bench_root/100k"
target/release/ferroseek-bench --files 1000000 --work "$bench_root/1m"
```

The stored `100k.md` and `1m.md` are complete harness reports. Corpus sizes
count generated files, including dotfiles excluded by default; the reports
also give actual indexed files/directories. Each size is one full lifecycle
run. Query microcases have five rounds of three requests per transport,
plus an untimed warmup. Every microcase response is checked for the exact
expected path/count (or empty result). Library errors, D-Bus errors, wrong
results and visibility timeouts fail the run. Burst search errors now fail
instead of being converted to zero hits.

The new sentinel is placed in `ferroseek-review-target/`, with filename
`report-review-sentinel.txt`. The selective cases search for `report` with
that path filter or `SearchOptions.within`. KRunner Match has no `within`
parameter, so that case is library-only. Path-only misses deliberately force
a full traversal. The fixture is removed and disappearance checked before
the incremental measurements. All filesystem and bus activity is isolated.

Library microcases reuse a reader and a parsed query; their timing covers
`Reader::search`, not connection creation or parsing. D-Bus timings include
request transport, parsing, search and result decoding.

Warmups prime caches. Each transport is measured separately; the figures
are absolute measurements, not an A/B comparison. Median/range refer to
five round medians, maximum to all 15 samples. CPU/wall uses the calling
thread clock around library requests; ratios for sub-microsecond slash-only
queries are dominated by timer overhead and must not be interpreted.

The ordinary 390-query set is a mix of queries and does not independently
verify expected results. The legacy relevance row counts skipped hidden
and non-UTF-8 samples as successes; it is retained verbatim in raw output
but **must not be interpreted as measured 100% relevance**. This remeasurement
uses the explicit checks in the new cases, not that row, for correctness.

Path-only SQL was checked outside the timed region with EXPLAIN QUERY PLAN:
`SELECT e.id,e.parent,e.name,e.key,e.kind,e.size,e.mtime FROM entries e`
reports `SCAN e`. Library CPU/wall near 1 on these cases supports a CPU-bound
scan/path-resolution cost, rather than bus transport or disk reads. No
profile-based attribution among SQLite decoding, allocation and path
construction is claimed.
