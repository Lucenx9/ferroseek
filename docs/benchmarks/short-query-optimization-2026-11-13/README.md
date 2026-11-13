# Short-query optimization measurements

The [probe](probe.rs) measures the public `Reader::search` interface on fresh
synthetic indexes with 10,000 and 100,000 files. It expands the
[initial diagnostic](../search-short-tokens-2026-11-13/README.md) with positive
short-token queries, short paths, root matches and ordinary-query controls.
No real filesystem tree, user index, session bus or application is accessed.

The fixture has one synthetic root and 100 directories, with uniformly
distributed `report-NNNNNN.txt` files. Each of eleven cases has one warmup and
five rounds of three timed calls at each size. Query order rotates between
rounds. The timer includes the read transaction but excludes query parsing,
verification, printing, process startup, live-path validation and rendering.
The result limit is 15. Fixed expected counts and paths check the fixture;
a fingerprint of ordered paths and scores additionally compares versions.

The [comparison runner](compare.py) alternates baseline and candidate order
over five process repetitions. It requires identical results for all 3,300
responses and a median reduction of at least 20% for both motivating 100k
no-hit cases. This is an opt-in optimization acceptance check, not an absolute
CI latency threshold or a physical-desktop performance claim. Ordinary controls
must also be inspected; the timing gate only covers the two motivating cases.

## Reproduce

Build the baseline (`25602fc`) and candidate in separate checkouts, using the
same Rust toolchain, locked dependencies and release profile. Compile this
same probe source against each library. Keep the resulting executables outside
the checkouts, then run the comparison with a new output directory:

```sh
# In each checkout, with probe_source pointing to this file's absolute path
# and probe_binary pointing to its baseline/candidate executable destination:
cargo build -p ferroseek-index --release --locked
rustc --edition=2024 -C opt-level=3 -C panic=abort "$probe_source" \
  --extern ferroseek_index=target/release/libferroseek_index.rlib \
  -L dependency=target/release/deps -o "$probe_binary"

# From the candidate checkout; the output directory must not exist:
python3 docs/benchmarks/short-query-optimization-2026-11-13/compare.py \
  /absolute/path/probe-before /absolute/path/probe-after /absolute/path/new-output
```

Use tmpfs for the recorded environment; do not compare a disk-backed run to
tmpfs or a debug build to release. Complete builds before starting measurements.
The runner retains each process's CSV and a summary even if a timing budget
fails. CSV fields are file count, round, sample, case, wall microseconds,
result count and ordered-result fingerprint. Summary medians and ranges are
computed from the five process medians, each based on fifteen samples. These
are warm-cache diagnostics on a non-dedicated host, not stable tail estimates.


## Recorded comparison — 2026-11-13

Baseline: `25602fc7f043d099544fcfc56cced7e716492ae1`.
Candidate: `e0ebc35ec9709a30bf4813c615a4c137ed52648e`.
Intel N95 (4 cores), Debian 13, Linux 6.12.116+deb13-amd64, Rust 1.94.0,
locked workspace release profile and tmpfs indexes. Builds finished before
measurement; no graphical harness ran concurrently. The host was not dedicated.

[Raw samples](samples.csv.gz) contain all 3,300 responses with a CSV header;
[summary.json](summary.json) contains both index sizes. Each value below is the
median of five process medians; brackets show their minimum and maximum.

| Case (100k files) | Before median [range], ms | After median [range], ms |
|---|---:|---:|
| exact | 0.910 [0.891–0.941] | 0.900 [0.892–0.916] |
| long-path-miss | 0.085 [0.083–0.090] | 0.082 [0.082–0.084] |
| mixed-hit | 4.277 [4.258–4.406] | 4.303 [4.270–4.358] |
| ordinary-miss | 0.059 [0.059–0.061] | 0.059 [0.059–0.074] |
| ordinary-prefix | 3.095 [3.061–3.148] | 3.070 [3.030–3.156] |
| path-hit | 4.234 [4.215–4.320] | 4.263 [4.195–4.307] |
| short-path-hit | 50.131 [49.730–50.512] | 37.898 [37.631–38.071] |
| short-path-miss | 44.815 [44.472–45.446] | 4.614 [4.534–4.688] |
| short-root-hit | 2.533 [2.530–2.608] | 2.543 [2.493–2.570] |
| short-tokens-hit | 2.871 [2.827–2.895] | 2.961 [2.899–2.981] |
| short-tokens-miss | 65.284 [64.833–65.780] | 24.811 [24.466–25.440] |

The motivating 100k no-hit cases improve by 89.7% (`zz/`) and 62.0% (`re zz`),
well beyond their observed process variance. Both pass the declared 20% gate.
The positive short path also improves; ordinary controls mostly overlap.
The matching short-token control (`re po`) costs about 0.09 ms more at 100k
(3.1%, with narrowly separated ranges) and 0.10 ms more at 10k. SQL predicate
setup/evaluation remains a tradeoff when the existing candidate budget fills
quickly. This targeted reduction in expensive rejected-row scans is accepted
with that measured cost; it is not a claim that every query gets faster.

All result counts and ordered path/score fingerprints agree. This fixture does
not prove equivalence for every tree; index tests separately cover policy filters,
Unicode, literals, tree mutations, oversized token lists and candidate budgets.
No end-to-end launcher latency or physical-desktop improvement is established.

### Experiment ledger

| Attempt | Observation | Decision |
|---|---|---|
| Centralize candidate filtering | Removes duplicated decoding/filtering paths; independently measured before optimization | Keep for ownership and maintainability, no separate speed claim |
| Push short name-token rejection into SQL | Isolated runs reduce the motivating token miss; final paired reduction is 62.0% | Keep; disclose the small matching-query cost above |
| Seed short paths from directory names | Isolated runs remove unrelated file-row/path decoding; final no-hit reduction is 89.7% | Keep |
| Emit every token into SQL | A 1,100-token regression fails with SQLite expression-depth error | Reject unbounded expressions; emit at most eight and check the remainder in Rust |
| Probe directories before checking a root-name match | Exploratory root-hit control regresses | Reject; preserve the root fallback before the probe |
| Recheck all name tokens after SQL filtering | Exploratory matching-token control pays duplicated work | Remove redundant checks; retain checks for tokens not emitted into SQL |
| Use the existing direct-scope budget for newly seeded short paths | A newer matching file displaces an old result that previously filled the streaming budget | Reject; preserve prior stage order and budgets, with a red/green regression test |
| Load root paths for unscoped name-only queries | Adds an unnecessary query to ordinary controls | Skip root loading when there is no path or explicit scope |

Intermediate timings informed these decisions but are not the accepted result;
the complete final paired run above measures the reviewed candidate. No schema,
index or cache was added. The positive-query cost is retained explicitly as a
tradeoff of the effective no-hit optimization, not credited as an improvement.

### Artifact identities

SHA-256:

```text
search.rs      7e1b007182e89a81d60af0cb858376d770647b087da22092869e6673996ba285
probe.rs       5795e84d35b9d97109af606ec115ad463871b5ed53b3ad08382bfb0b159511ca
baseline probe 7e9bb36524714c0f1d7a121d41f76f84401bb4ac891355f7f00dd49b73851d1a
candidate probe 299550b21ff242046c646b1b0a952e14152e15513a69f6c7f8d82a6ed0d0e1a5
```

Executable hashes identify the measured local artifacts; build paths and linker
metadata may prevent byte-identical rebuilds. The source commits, probe and locked
dependencies define reproduction. Use the same probe for both revisions.
