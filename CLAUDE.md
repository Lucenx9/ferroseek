# FerroSeek: notes for agents

A Rust filename search daemon with a KRunner D-Bus runner. Design and
evidence are in `docs/ARCHITECTURE.md`, measurements in
`docs/BENCHMARKS.md`, Baloo/KRunner source analysis in `docs/RESEARCH.md`.

## Map

- `crates/index`: SQLite store. `Writer`/`Txn` mutate the tree; `Reader`
  searches. Schema, versioning and recovery are in `schema.rs`, ranking in
  `search.rs`. Only this crate knows SQL.
- `crates/engine`:
  - `config.rs`: config, XDG paths, exclusion `Rules`
  - `inotify.rs`: raw libc binding
  - `indexer.rs`: the core; `reconcile_dir` and `resolve` are the only
    places that compare the filesystem with the index
  - `engine.rs`: threads, debounce, pause and duty cycle
- `crates/daemon` (`ferroseekd`): `runner.rs` (`org.kde.krunner1`),
  `control.rs` (`io.github.ferroseek.FerroSeek1`), `open.rs` (launching
  files), `mime.rs` (globs2 → icon).
- `crates/cli` (`ferroseek`), `crates/bench` (`ferroseek-bench`).
- `crates/mcp` (`ferroseek-mcp`): optional read-only MCP stdio server (rmcp 3.8).
  `policy.rs` (allowlist/deny/path validation, pure) and `server.rs` (tools,
  reader cache, idle release). See `docs/MCP.md`.
- `data/`: KRunner `.desktop` file, D-Bus `.service` file, systemd unit
  templates (`@BINDIR@`). `scripts/`: install and uninstall.
- `prototype/backend-bench`: the throwaway backend comparison. Not in the
  workspace; keep it as evidence.

## Commands

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace && tests/dbus-smoke.sh target/release/ferroseekd
target/release/ferroseek-bench --files 100000 --work ~/.cache/ferroseek-bench-100000
```

## Rules

- **Tests never touch the real home directory, index or session bus.**
  - Use `tempfile` dirs and `Paths { .. }`, or the `FERROSEEK_CONFIG` and
    `FERROSEEK_DATA_DIR` env vars.
  - D-Bus tests run under `dbus-run-session` (the smoke script and the bench
    re-exec themselves).
  - CLI tests set `DBUS_SESSION_BUS_ADDRESS` to a dead socket.
- **Never modify KDE or Baloo configuration, and never run `balooctl`
  commands that change state.** Live desktop tests install with
  `scripts/install.sh --config <tmp config>` and are undone with
  `scripts/uninstall.sh` without purging user data. Route all live-test data
  into a private temporary directory and remove only test-created artifacts.
- **Index changes are state-based.** Event handlers record `(dir_id, name)`
  as dirty, and `resolve()` `lstat`s the current path. Never apply event
  deltas blindly. Never delete on `ENOENT` unless the parent still has its
  stored inode (otherwise it is a stale path from an unapplied rename).
- **Directory `mtime` in the index means "listing reconciled at".** Only
  `Txn::mark_dir_reconciled` sets it, and quick reconciles rely on it.
- **Performance claims need a measurement in `docs/BENCHMARKS.md`.** Keep
  the environment table and the commands current. No Baloo comparisons
  without running §6 of that file.
- **zbus is v6.** Check the sources in `~/.cargo/registry` rather than
  memory:
  - features are `default-rt`, `proxy`, `service`
  - there is no blocking module; `zbus::block_on` drives sync code
  - `zruntime::unblock` runs blocking work
- `gen` is a reserved keyword in edition 2024.
- The KRunner contract (method signatures, property keys, `.desktop` keys)
  is pinned to KRunner 6.31 in `docs/RESEARCH.md` §10. Re-verify it when
  bumping Plasma.
- Commits end with the `Co-Authored-By` trailer in use in `git log`.

## Review checkpoint (2026-10-31)

- Completed initial whole-tree Standards/Spec review; six findings fixed.
  See `docs/CODE_REVIEW.md` for findings, regressions and tradeoffs.
- Policy changes invalidate directory listings transactionally through
  `Writer::apply_policy`; preserve crash-safe invalidation on future changes.
- Inotify reads must return after one buffer so channel backpressure works.
- Search budgets count candidates *after* filters. Path-only queries stream
  entries; do not infer a mandatory filename token from a directory path.
- Existing benchmark numbers predate these fixes; selective/path-only query
  latency and cache-tag checking overhead have not been rebenchmarked.

## Remeasurement checkpoint (2026-10-31)

- Review fixes measured at 100k and 1M generated files; see BENCHMARKS.md §8
  and `docs/benchmarks/review-2026-10-31/` for complete raw reports/hashes.
- `crates/bench/src/review.rs` adds checked path/filter cases: 5 rounds × 3
  requests per transport. 330 measured results checked across both sizes.
- At 1M, path-only D-Bus median ≈524 ms, path+name ≈110 ms: known optimization
  target. Ordinary mixed-query D-Bus median 2.53 ms; steady idle RSS after
  restart 9.4 MiB. These are warm-cache measurements, not before/after gains.
- Legacy relevance percentages count skipped samples as successes; do not
  cite them as accuracy evidence. The limitation is explicit in the report.

## Path optimization checkpoint

- Path searches derive bounded directory scopes from existing name FTS and
  parent indexes; no schema change. Root/broad anchors fall back; short path
  components may still scan. Preserve explicit prefix ordering before caps.
- All search stages share one read transaction. Tests compare independent
  path lists and exercise concurrent directory renames.
- Rust minimum is 1.89 (locked dependencies require it). Raising it enables
  Clippy's let-chain suggestions, applied without behavioral changes.

## Accepted optimization and release checkpoint (2026-10-31)

- Production commits: e8e8b85 (bounded directory scope), a4c33c8 (daemon
  lifecycle/installer hardening and native FTS ordering). Paired measurements
  against preserved 6f7172d binaries are in BENCHMARKS.md §9.
- At 1M: selective path-only 529.820 → 2.032 ms, path+name 109.939 → 2.069 ms
  across five independent daemon processes per side. No new schema/index.
  Broad/short-component fallbacks remain. Mixed-query p95 is still 24.80 ms.
- Benchmark library rows use the harness library even with an old daemon;
  compare D-Bus rows only. Legacy relevance skipped-sample denominator fixed.
- Actual Plasma activation/results/Run/open-parent/live updates/restart and
  repeated installation/uninstallation verified using isolated test data.
  Baloo stayed disabled; no installed test service remains.
- Reader-pool epoch and database identity checks protect Teardown/recovery.
  Never restore per-Status malloc_trim or blocking async control work.
- Benchmark --work must be new/empty unless explicitly reusing a matching
  generated corpus. Reuse initial indexing deletes only that work/data.

## Filesystem recovery checkpoint

- CI stable was Rust 1.99; reproduce its Clippy checks with that toolchain,
  not only the local default (1.94). Removed a newly diagnosed redundant borrow.
- Incomplete listings and failed child stats must never imply deletion.
  Preserve existing entries, count errors and invalidate directory listing
  timestamps so future quick passes retry. Event stat errors enter bounded
  retry handling. Overflow and dropped-event recovery must use Full mode:
  changing file contents does not update its parent directory's mtime.
- `scripts/verify-index.py` is an independent read-only desktop oracle. Use
  complete effective config; reports contain counts, not private file paths.
  See docs/DESKTOP_SOAK.md. Never commit real user indexes/configs/logs.

## MCP checkpoint (2026-10-31)

- `ferroseek-mcp` must stay read-only, stdio-only and opt-in. It must never
  start an indexer, write the index or add a schema change. Its only index
  API addition is `Reader::roots()`.
- Every path leaving the process passes `Policy::visible_under` *and* lies
  inside the requested scope. Client paths are validated, not normalized;
  never echo refused paths in errors.
- Structured outputs must satisfy their `outputSchema`: Claude Code
  rejects responses with missing `required` fields. Fields omitted by serde
  need `#[serde(default)]`; the protocol test checks this.
- Live agent checks use `claude -p --mcp-config … --strict-mcp-config` and
  `codex exec -c mcp_servers.…`, with temp synthetic data. Never register
  test servers in the user's agent configs.
- `tests/mcp-smoke.py` and `scripts/benchmark-mcp.py` (results in
  BENCHMARKS.md §10).
