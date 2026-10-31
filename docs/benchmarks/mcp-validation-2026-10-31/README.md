# MCP validation: filename tools, freshness and stale recovery

Measured 2026-10-31 on one Linux desktop: Ryzen 7 5800X, 16 logical CPUs,
31 GiB RAM, NVMe/Btrfs. Rust 1.94 release builds; stable Rust 1.99 Clippy
and MSRV 1.89 checked separately. Native tools: fd 10.5.0, ripgrep 15.2.0,
plocate 1.1.25. The desktop and its 72-hour stability test kept running.
No global cache drops, live-index mutations, KDE changes or Baloo activation.

Baseline is `c46010e3f5be3b9f6aae44f001b961c5cfd0b3ae`; candidate production
code is `fba05d1`. Baseline binaries were frozen before editing. The four MCP
commits were published without rewriting history; their full
[GitHub CI run](https://github.com/Lucenx9/ferroseek/actions/runs/41230250537)
passed, including current-stable Clippy and the MSRV job.

## Scope and method

Four generated repositories contain 25,000 regular source files each, plus
Cargo.toml and main.rs. A fifth repository is a public `git archive c46010e`
snapshot of FerroSeek. An indexed unauthorized sibling tests confinement.
No private repository, filenames, config, index or desktop log is published.
The realistic corpus is one small source tree, not a broad workload survey.

The common subset is case-insensitive ASCII literal substrings, AND across
tokens: tokens without `/` match basenames; tokens with `/` match paths.
Only non-hidden regular files are expected; no content search or symlink
traversal. These benign fixtures disable MCP's additional default deny names;
real installations retain them. No query hits the 200-result/size limit.
The independent oracle enumerates the filesystem and applies those predicates.
Each tool sees the same authorized roots. MCP sessions are scoped per case;
fd/rg receive those roots; plocate output is filtered by those roots and live
existence before returning. The private updatedb database scans the containing
benchmark work directory (including its few control files); it is not the
system database. This adapter prevents unauthorized results but does not make
plocate's native access policy equivalent to MCP's security policy.

fd uses fixed-string basename search; `rg --files --iglob` uses a selective
literal-derived glob. Neither scans file contents. plocate uses a selective
basename token, followed by the same path predicates. Predicate adaptation,
decoding, existence checks and CLI process startup are timed. MCP measures
actual stdio JSON request through decoded structured response, with a resident
server; initialization is separate (median 1.32 ms). It additionally returns
metadata and ranking. This compares these invocation patterns, not identical
APIs or a hypothetical persistent fd/rg server. The measured PATH `rg` is a
small `exec /usr/bin/rg` shell wrapper; its launch cost is included.

Five repetitions recreate private indexes and start fresh MCP processes per
case; each has warmups and five timed queries per tool, with rotated tool
order. Dataset and filesystem cache are reused. **Warm-cache measurements**:
no cold-storage claims. Fresh processes are not cold filesystem caches.
Background desktop activity, order effects and one machine limit inference.

## End-to-end query latency

Milliseconds: median of the five run medians, with their min–max range.
Lower observed latency in this table does not establish a universal winner.
All 700 measured steady-state responses exactly matched the independent oracle
(100% recall, zero false positives; empty-query-result recall defined as 1).

| Case | MCP | fd | rg --files | plocate |
|---|---:|---:|---:|---:|
| single-exact | 1.609 (1.464–1.909) | 8.622 (8.234–9.125) | 6.227 (6.076–6.687) | 1.889 (1.848–2.008) |
| multi-exact | 4.730 (4.409–4.949) | 12.395 (11.737–13.073) | 11.467 (11.158–12.118) | 1.980 (1.916–2.009) |
| single-path | 3.192 (3.069–3.295) | 9.939 (9.135–10.168) | 6.628 (6.427–6.700) | 3.605 (3.488–3.714) |
| multi-path | 3.604 (3.489–4.092) | 12.417 (12.016–12.550) | 10.000 (9.812–10.920) | 1.989 (1.915–2.070) |
| multi-cargo | 1.036 (0.993–1.110) | 11.774 (11.704–12.437) | 11.491 (11.311–12.237) | 2.051 (1.936–2.073) |
| real-repository | 0.506 (0.457–0.547) | 6.842 (6.646–7.145) | 3.915 (3.833–4.100) | 1.760 (1.728–2.032) |
| missing | 0.504 (0.491–0.542) | 11.296 (10.368–11.526) | 11.070 (10.845–11.336) | 1.847 (1.782–1.914) |

Cases: single-exact `module000501.rs` in one generated repository;
multi-exact in four; single-path `group005/ module0005` (100 matches);
multi-path `group005/ module000501` in four; multi-cargo `Cargo.toml` in all
five; real-repository `main.rs` in the public snapshot; missing
`no-such-fixture.rs` in all five. “Exact” denotes the fixture's full filename,
not a different equality matching rule. This run does not measure pure
path-only searches without a filename token.

Pooled p50/p95/p99 below are descriptive nearest-rank quantiles over 25 calls
per cell. Repeated calls within a process are correlated. With only five
independent process runs, p99 is essentially the observed maximum; these are
not reliable production tail estimates or confidence intervals.

| Case | Tool | p50 ms | p95 ms | p99 ms | Child CPU/call ms | Adapter CPU/call ms |
|---|---|---:|---:|---:|---:|---:|
| single-exact | mcp | 1.605 | 1.924 | 2.935 | see batch | 0.058 |
| single-exact | fd | 8.622 | 9.517 | 9.988 | 36.204 | 0.327 |
| single-exact | rg | 6.227 | 7.337 | 7.412 | 24.463 | 0.310 |
| single-exact | plocate | 1.891 | 2.059 | 2.459 | 1.683 | 0.292 |
| multi-exact | mcp | 4.700 | 5.279 | 5.335 | see batch | 0.064 |
| multi-exact | fd | 12.389 | 13.993 | 14.050 | 89.266 | 0.381 |
| multi-exact | rg | 11.467 | 12.764 | 12.823 | 90.115 | 0.361 |
| multi-exact | plocate | 1.977 | 2.123 | 2.220 | 1.709 | 0.320 |
| single-path | mcp | 3.192 | 3.403 | 3.481 | see batch | 0.170 |
| single-path | fd | 9.803 | 11.134 | 11.264 | 36.479 | 1.076 |
| single-path | rg | 6.663 | 7.790 | 7.831 | 21.622 | 1.054 |
| single-path | plocate | 3.617 | 3.893 | 3.907 | 2.312 | 1.353 |
| multi-path | mcp | 3.703 | 4.346 | 4.407 | see batch | 0.064 |
| multi-path | fd | 12.374 | 13.050 | 13.758 | 89.406 | 0.369 |
| multi-path | rg | 10.013 | 11.346 | 11.535 | 72.569 | 0.363 |
| multi-path | plocate | 1.989 | 2.127 | 2.955 | 1.684 | 0.325 |
| multi-cargo | mcp | 1.093 | 1.212 | 1.270 | see batch | 0.072 |
| multi-cargo | fd | 11.774 | 12.776 | 12.819 | 85.026 | 0.438 |
| multi-cargo | rg | 11.560 | 13.439 | 14.874 | 89.359 | 0.441 |
| multi-cargo | plocate | 2.051 | 2.235 | 2.256 | 1.704 | 0.397 |
| real-repository | mcp | 0.494 | 0.582 | 0.605 | see batch | 0.062 |
| real-repository | fd | 6.842 | 7.652 | 8.344 | 12.014 | 0.346 |
| real-repository | rg | 3.930 | 5.036 | 5.219 | 3.929 | 0.325 |
| real-repository | plocate | 1.809 | 2.041 | 2.346 | 1.514 | 0.316 |
| missing | mcp | 0.504 | 0.588 | 0.609 | see batch | 0.054 |
| missing | fd | 11.231 | 11.875 | 12.179 | 71.170 | 0.277 |
| missing | rg | 11.132 | 12.029 | 12.160 | 88.687 | 0.301 |
| missing | plocate | 1.816 | 2.013 | 2.041 | 1.600 | 0.258 |

MCP server CPU is measured from /proc ticks (10 ms resolution) across five
requests per case, separately from client CPU above. Median batch CPU was:
- single-exact: 0 ms per five requests.
- multi-exact: 20 ms per five requests.
- single-path: 10 ms per five requests.
- multi-path: 20 ms per five requests.
- multi-cargo: 0 ms per five requests.
- real-repository: 0 ms per five requests.
- missing: 0 ms per five requests.

### Native tuning and memory

Separate five-rotation correctness-checked trials tested 1, 2, 4 and automatic
threads (`tuning.json`, 280 responses). Automatic threads gave the lowest
median on the multi-repository cases for both tools. Tiny repositories
benefited from one thread: fd 3.34 ms and rg 2.75 ms versus default 6.70/4.66 ms
in those tuning trials. fd's single-exact median improved from 8.92 to 7.19 ms
with four threads. Tuning results are a separate run, not substituted into the
main table. Threading trades latency for CPU and memory; defaults are not
optimal for every case.

MCP's largest observed active VmRSS/VmHWM across the small-corpus cases was
9,200 KiB. After the final missing-query workload, a 12-second idle wait gave
median RSS 7,524 KiB (range 7,496–7,580), two threads. Five subsequent two-second
observations recorded zero CPU ticks and context switches. This short window
cannot prove zero CPU for longer periods or all preceding workloads.

Independent multi-exact memory-only trials sampled post-exec VmHWM: fd median
83,776 KiB, rg 7,028 KiB, plocate 5,600 KiB. Sampling every 0.5 ms can miss the
final high water mark, especially plocate (only 2–3 samples). These are
process-only observations; MCP daemon/indexer and the Python client are
separate. Native wait4 `peak_rss_kib` in other raw files can include the
inherited Python pre-exec image and **must not be interpreted as native RSS**.
The separate sampling file corrects that measurement limitation.

All native query `ru_inblock`/`ru_oublock` values and MCP query physical
`read_bytes`/`write_bytes` were zero in this warm-cache run. Logical stdio I/O
and syscall counts are retained in the raw data. Zero storage I/O does not
mean no filesystem/cache work. CPU totals for parallel tools can exceed wall
time.

## Indexing and maintenance costs

Use **index-costs.json** for costs: both private indexes were removed before
each of five initial runs, with rotated tool order. Earlier `comparison.json`
maintenance rows reused plocate's DB after the first run and are **not** fair
fresh-build evidence; retained raw data is explicitly superseded here.
One probe rename is verified before/after each explicit refresh.
FerroSeek uses foreground full reconciliation; plocate uses updatedb. This
is separate from event-driven incremental maintenance. fd/rg have no persistent
index construction cost and enumerate on every search.

Medians; output blocks are Linux wait4 block counts, not measured physical
NVMe traffic after filesystem compression/coalescing. Initial reads were all
cache hits. Database sizes are final index files, not peak disk footprint.

| Tool | Operation | Wall ms | CPU ms | DB bytes | Output blocks |
|---|---|---:|---:|---:|---:|
| ferroseek | initial | 676.12 | 648.38 | 19030016.00 | 88032.00 |
| ferroseek | rename-refresh | 233.24 | 228.17 | 19030016.00 | 1048.00 |
| plocate | initial | 122.35 | 121.65 | 667009.00 | 1304.00 |
| plocate | rename-refresh | 124.18 | 123.45 | 516196.00 | 1112.00 |

MCP itself is read-only and does not maintain a second index. Its schema and
persistent index size are unchanged by the stale fix. FerroSeek's database
stores metadata/tree relationships as well as search names; plocate's smaller
name database has a different feature set. updatedb's refreshed file size
also varies with private work-directory contents/order.

### Recently changed files

Five stopped-index trials: new-file recall was 0 for MCP/plocate and 1 for
fd/rg before refresh; all were 1 after explicit refresh. Deleted files were
excluded by every timed adapter, including plocate's live-existence filter.
An index that is not maintained cannot discover new names.

A **private dbus-run-session daemon**, not the running desktop service,
performed five independent sessions of five creates, renames and deletes
(25 operations per kind). MCP was polled every 5 ms; a separate read-only SQL
check verified actual index convergence. Timings include polling granularity.
Deletion can disappear from MCP immediately through live validation, before
the underlying index catches up.

| Change | MCP visibility p50/p95/p99 ms | Index convergence p50/p95/p99 ms |
|---|---:|---:|
| create | 202.67/206.95/207.28 | 202.77/207.13/207.47 |
| rename | 203.65/206.56/209.20 | 203.69/206.60/209.24 |
| delete | 0.57/0.73/0.82 | 203.44/204.47/204.54 |

Daemon resource deltas for each 15-operation session: median CPU 10 ms
(0–10, tick resolution); median write_bytes 610,304 (569,344–1,204,224),
read_bytes 0. Median observed RSS 16,756 KiB, median VmHWM 17,588 KiB.
These include watcher/index maintenance, not just MCP calls. No private daemon
warnings/errors were observed. These five short sessions are not the ongoing
72-hour stability test and do not establish long-term reliability.

## The 8,192 stale-candidate defect and fix

Independent regression fixture: index 8,300 high-ranked `needle.txt` entries
and two lower-ranked live names, then delete the former subtree. Baseline
returned zero survivors, reporting no more results. The candidate filters
vanished paths inside the index's accepted-candidate budget on the recovery
path. Authorization precedes disk access, and metadata uses the existing
no-symlink traversal helper. The ordinary path does not stat all candidates.

Five process pairs, five warm calls per side: baseline recall **0/2**, candidate
**2/2**, every time. Median of run medians: baseline 172.32 ms, candidate
93.89 ms. These are **not equivalent successful operations**, so this is a
correctness repair, not a valid speedup claim. Regression tests also cover
limits 1 and 10, with and without an explicit subdirectory restriction.

`omitted_stale` now documents a lower bound. Concurrent disappearance between
candidate filtering and response generation still has bounded retries; when
exhausted, `incomplete_due_to_changes` tells clients to refine/retry. It is not
a globally atomic filesystem snapshot or a detector of every concurrent race.
No new persistent index, retained metadata cache, allocator change or schema
migration was introduced. The fallback may do substantial disk metadata work
on huge stale populations; regular maintenance remains necessary.

## Million-file scoped investigation

Five baseline/candidate process pairs, seed 7, 100 filename substrings, both
whole-corpus and `Personal` subtree. Ten warmups per scope. Fixed baseline-first
AB order (not randomized) is an order-bias limitation. Same synthetic index
and filesystem on both sides. Every returned path was checked independently
for existence, query predicate and scope; ordered output matched baseline in
all 2,000 calls. This equivalence test does **not** establish exhaustive recall
or globally exact ranking for large result sets. A separate manual expected
ordering regression covers depth-adjusted ranking under scoped policy filters.

| Scope/build | Median of run medians ms | Pooled p50/p95/p99 ms |
|---|---:|---:|
| whole corpus/baseline | 6.852 (6.718–7.078) | 6.807/12.987/15.297 |
| whole corpus/candidate | 6.865 (6.672–7.081) | 6.934/12.953/15.928 |
| subtree/baseline | 9.401 (9.161–9.749) | 9.421/89.085/98.155 |
| subtree/candidate | 9.486 (9.320–9.771) | 9.486/89.360/95.318 |

No meaningful ordinary-query latency change is demonstrated; the scoped tail
remains a bottleneck (~89 ms p95 here). SQL EXPLAIN shows key-index lookup for
exact/prefix names, FTS virtual-table search plus rowid lookup for substrings,
and the parent index for recursive descendants. FTS `SCAN ... VIRTUAL TABLE`
does not mean a full entries-table scan. The scope has 2,194 directories;
`directory_scope` intentionally abandons expansion above 256 directories,
then streams global filename candidates and checks paths. Some sampled tokens
have ~59–61k global FTS matches (diagnostic full counts, not necessarily rows
actually consumed before the candidate cap). This supports candidate/path
filtering as a remaining cost; it is not a per-function CPU profile.

An experiment extended the exact-score stopping bound to scoped stage-two
ranking. Five paired runs did not demonstrate a convincing gain (scoped
median of run medians 9.460 → 9.435 ms, overlapping run ranges); it was removed.
`pairs.json` is explicitly **rejected-experiment evidence**, not the final
candidate. `final-pairs.json` measures only the accepted stale fix. No second
indexing system or speculative persistent cache was added.

## Reproduction

Use a fresh disposable work directory and output files; commands deliberately
avoid the desktop index and service. Linux /proc, Python 3.12+ (tar extraction
filter), fd, rg, plocate/updatedb and dbus-run-session are needed for all rows.
The harness tolerates missing plocate in the main comparison only; omit the
plocate-specific supplemental runs when unavailable. Python assertions must
remain enabled. These are benchmark harnesses, not general-purpose tool
wrappers; matching equivalence is limited to the documented fixture queries.

```sh
cargo build --release --locked --workspace
# Preserve a baseline binary built from c46010e before building the candidate.
# A separate ordinary clone with CARGO_TARGET_DIR avoids touching this checkout.
# Set BASELINE_MCP to that baseline executable.
work="$HOME/.cache/ferroseek-mcp-comparison-new"
mkdir -p bench-out/mcp-validation-new
out=bench-out/mcp-validation-new
python3 scripts/benchmark-search-tools.py --source-ref c46010e   --work "$work" --output "$out/comparison.json" --queries-per-case 5
python3 scripts/benchmark-search-tuning.py --work "$work" --output "$out/tuning.json"
python3 scripts/benchmark-native-memory.py --work "$work" --output "$out/native-memory.json"
python3 scripts/benchmark-mcp-maintenance.py --work "$work" --output "$out/maintenance.json"
python3 scripts/benchmark-index-costs.py --work "$work" --output "$out/index-costs.json"
python3 scripts/benchmark-mcp-stale.py --baseline "$BASELINE_MCP"   --candidate target/release/ferroseek-mcp --work "$work-stale" --output "$out/stale.json"
# Reuse only a generated benchmark corpus, never a user index:
# target/release/ferroseek-bench --files 1000000 --work "$work-million" > "$work-million-report.md"
# Copy that generated report to "$work-million/report.md" for the safety marker.
python3 scripts/benchmark-mcp-pairs.py --baseline "$BASELINE_MCP"   --candidate target/release/ferroseek-mcp --work "$work-million" --output "$out/final-pairs.json"
python3 scripts/benchmark-mcp-query-plan.py --work "$work-million" --output "$out/query-plan.json"
```

The public source snapshot ref is pinned for comparable dataset topology.
Generated file mtimes, cache state, machine load and path lengths still affect
reproduction; no byte-identical timing/size claim. The read-only million-file
run reused the existing generated 1M corpus from the earlier benchmark rather
than rebuilding it. JSON query IDs map to seed-7 selection from that index;
its SQL row IDs depend on corpus creation. Rebuilding can change the sampled
queries, so compare both builds against the **same** newly created corpus.

All accepted raw measurements and the rejected experiment are adjacent JSON
files, covered by SHA256SUMS. `comparison.json`'s historical RSS note is
superseded by the measurement caveat above; no raw measurements were rewritten.
Scripts now remove plocate's database before each initial run and support a
pinned public source ref; the separate corrected cost run supplies this report.

## Validation, security and remaining limits

108 Rust tests passed; fmt, stable-1.99 Clippy with warnings denied,
MSRV-1.89 all-target checks, workspace release builds and installation,
benchmark, independent-index-oracle, MCP (four protocol versions) and private
D-Bus smoke checks passed. Review preserved read-only access, explicit
allowlists, bounded responses, authorized-before-stat ordering, no-symlink
metadata access and stdio-only transport. No production daemon/engine/schema
code changed in this milestone.

No release was created. Main remaining limits: broad/scoped latency tails,
approximate ranking under existing candidate caps, no cold-cache evidence,
one machine and one small realistic repository, short repeated-query samples,
coarse CPU/peak-memory observations, and the still-running desktop soak.
These results do not establish Baloo compatibility or overall superiority over
fd, rg or plocate. The installed desktop daemon retained its original PID;
its service, timer, config and live index were not modified by these tests.
