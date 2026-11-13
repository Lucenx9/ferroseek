# Short-token search diagnostic — 2026-11-13

Source: `25602fc7f043d099544fcfc56cced7e716492ae1`; no production code changes.
Intel N95 (4 cores), Debian 13, Linux 6.12.116+deb13-amd64, Rust 1.94.0.
The index library uses the locked workspace release profile. The small caller
uses `rustc -C opt-level=3 -C panic=abort`. SQLite databases were on tmpfs.

This measures `Reader::search`, including its read transaction, on synthetic
index rows. It excludes query parsing, process startup, filesystem scanning,
live-path validation, D-Bus, UI and rendering. No real user data or bus was used.
There is one synthetic root, 100 direct child directories and either 10,000 or
100,000 files named `report-NNNNNN.txt`, distributed evenly across directories.
The result limit is 15; other search options use their defaults.

One process created both indexes. Each case had one warmup followed by five
rounds of three requests; case order rotates between rounds. Every response
was checked: exact-name searches return the one independently specified path,
and the other queries return no matches. All 120 measured responses passed.
[samples.csv](samples.csv) has no header; fields are file count, round, sample,
case, wall microseconds, result count. [probe.rs](probe.rs) is the complete caller.

| Query | Files | Median (ms) | Observed min–max (ms) |
|---|---:|---:|---:|
| `report-000000.txt` | 10,000 | 0.511 | 0.481–0.742 |
| `report-000000.txt` | 100,000 | 0.908 | 0.864–1.524 |
| `zz/` | 10,000 | 4.309 | 4.269–6.052 |
| `zz/` | 100,000 | 45.698 | 44.307–56.538 |
| `missing/` | 10,000 | 0.070 | 0.046–0.222 |
| `missing/` | 100,000 | 0.081 | 0.073–0.202 |
| `re zz` | 10,000 | 6.438 | 6.237–9.015 |
| `re zz` | 100,000 | 67.899 | 66.371–84.600 |

The short-path query cannot seed `directory_scope` through a three-character
trigram; it streams all entries and checks their paths. Multiple short name
tokens likewise require the full-entry fallback, with an additional prefix
pass here. Rejected rows do not consume candidate budgets. The longer missing
directory can use the existing FTS directory probe and reject the scope early.
See [candidate generation](../../../crates/index/src/search.rs) for both paths.

These measurements identify full-scan query shapes worth profiling further;
they do not establish a regression, a proposed optimization's benefit, or desktop
latency. Fifteen samples from one process are not independent process trials or
stable tail-percentile estimates. Caches were warm, no eviction was attempted,
and the host was not dedicated. No performance optimization was attempted or kept.
Any future change must preserve substring semantics, filtering before budgets,
ranking and one read snapshot; simply capping rows examined would lose matches.

## Reproduce

Run from the repository root. Each execution needs a new work directory.

```sh
cargo build -p ferroseek-index --release --locked
probe_work=$(mktemp -d)
rustc --edition=2024 -C opt-level=3 -C panic=abort \
  docs/benchmarks/search-short-tokens-2026-11-13/probe.rs \
  --extern ferroseek_index=target/release/libferroseek_index.rlib \
  -L dependency=target/release/deps -o "$probe_work/search-probe"
"$probe_work/search-probe" "$probe_work" > "$probe_work/samples.csv"
```

The recorded library SHA-256 was
`c7eb68ded5f9cedf3cd71e0f0395a1b36b6a5ad66331142b8f494fffd234ec96`.
The caller binary SHA-256 was
`385e51aa0668f4ba66a7e510a4273744c9307954fd613fa41c5394365ca3fb32`.
Build paths and toolchains can change binary hashes.
