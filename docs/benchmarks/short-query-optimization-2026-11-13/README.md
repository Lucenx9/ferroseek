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
