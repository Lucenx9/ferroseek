# MCP access modes: regression measurement

Measured on 2026-10-31 on the machine described in
[the benchmark environment](../../BENCHMARKS.md#1-environment), using Rust
1.94.0 release builds. The private corpus and database were in `/tmp`
(**tmpfs**), not on the NVMe filesystem. This measures warm-cache policy and
protocol costs, not storage throughput. No live daemon or user index was used.

## Method and correctness

The baseline executable was the installed build preceding the access-mode
change, with restricted authorization. Binary SHA-256 identities are recorded
in [results.json](results.json). The candidate was built from the code committed
alongside this report. Each run had identical index data and authorized
coverage: two roots with 10,000 files each, named `item000000.txt` through
`item009999.txt`. Restricted authorized the common parent; HOME was that same
parent in home mode. No files outside these roots were indexed.

Five fresh processes per variant, rotated variant order, four warmup requests
per process, then ten repetitions of four searches (200 measured round trips
per variant, 800 total). Queries were `item000123`, `item00012`, `item009999`
restricted to one root, and `no-such-file`. Expected sets (2, 20, 1 and 0 paths)
were calculated independently from fixture names, and checked on every measured
response, including truncation and incomplete-response flags. **Zero mismatches.**

Latency spans Python client JSON serialization, stdio transport, Rust server
execution and client JSON decoding. It excludes process startup and indexing.
CPU is server user+system time across 40 measured queries, sampled from `/proc`
at 10 ms resolution. RSS is a single sample after those queries, not peak or
post-idle memory. Resources used by the Python client are excluded.

## Results

All latency values below are milliseconds. Pooled percentiles use the nearest
rank by `round((n - 1) * p)` across the 200 samples per variant. Run medians
summarize the five separate processes; queries within a process are correlated.

| Variant | Median of run medians | Run-median range | Pooled p50 | p95 | p99 | Observed RSS KiB |
|---|---:|---:|---:|---:|---:|---:|
| Previous restricted binary | 0.440 | 0.430–0.471 | 0.448 | 0.703 | 0.771 | 7984–8168 |
| New restricted | 0.440 | 0.432–0.487 | 0.447 | 0.685 | 0.751 | 7896–8032 |
| New all_indexed | 0.454 | 0.438–0.467 | 0.457 | 0.808 | 0.893 | 7968–8092 |
| New home | 0.434 | 0.414–0.464 | 0.433 | 0.688 | 0.749 | 7984–8076 |

Server CPU per 40-query batch was 10 ms for every sample. This coarse
counter cannot resolve per-query CPU differences.
Median ranges overlap, but global p95/p99 were higher in this sample. Global
mode checks ancestors from `/`, adding metadata traversal work compared with
an explicit parent boundary. This is a plausible contributor, not a measured
attribution. No statistical significance test was performed and this report
claims neither a speedup nor equivalent performance across workloads.

## Reproduce

Preserve the old executable before rebuilding, or build the parent revision
`50f42ff` in a separate checkout with the same toolchain. Then, from this revision:

```sh
cargo build --release --workspace --locked
python3 scripts/benchmark-mcp-access.py \
  --baseline /absolute/path/to/previous/ferroseek-mcp \
  --candidate target/release/ferroseek-mcp \
  --output /tmp/ferroseek-access-results.json
```

The output must not already exist. `TMPDIR` can select a different filesystem;
record that change when comparing results. The script uses a private HOME,
configuration, database and dead session-bus address. It creates and removes
only its temporary dataset. Raw output includes hashes and synthetic timing
rows, with no user paths or search results.

## Limits and validation

This is a small, selective warm-cache regression check. It does not establish
million-file tail latency, cold-cache behavior, peak/idle memory, disk I/O,
indexing maintenance cost or desktop responsiveness. No engine/index schema
changes were made. Earlier larger benchmark reports retain their original
scope; they are not measurements of this new policy.

Protocol regressions cover all three scopes, newly indexed roots becoming
visible to an already running global server, non-indexed and engine-excluded
files remaining inaccessible, default secret protections, hidden names,
custom deny precedence over exceptions and invalid startup modes/HOME.
Existing legacy allowlists retain their tests. Installer tests check explicit
global consent, refusal at EOF and dry-run behavior in isolated homes.

Review focused on policy-before-output enforcement, indexed lookup before
metadata access, CLI/config precedence, fail-closed parsing, no-symlink
metadata traversal, and installation consent before configuration writes.
Existing candidate budgets and approximate ranking for broad queries remain
unchanged. No live service, engine roots or existing client registrations
were modified during development.

Final validation: 111 Rust workspace tests passed; formatting and Clippy
(Rust 1.99.0, warnings denied), release workspace builds and Rust 1.89.0
compatibility checks passed. Installer smoke tests, four-component archive
tests and MCP protocol smoke tests passed in isolated environments.
