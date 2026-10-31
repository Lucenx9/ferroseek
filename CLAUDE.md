# FerroSeek: notes for agents

FerroSeek is a desktop-independent Rust filename/path search engine for
Linux. The core CLI, optional KDE KRunner D-Bus adapter and optional MCP
stdio server share one index. Design is in `docs/ARCHITECTURE.md`, measured
results in `docs/BENCHMARKS.md`, installation in `docs/INSTALLATION.md`,
and historical Baloo/KRunner research in `docs/RESEARCH.md`.

Documentation should lead with FerroSeek's own capabilities, clearly
distinguish implemented adapters from future ones, and keep performance
claims tied to their measured workloads. Do not imply Baloo feature parity.

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
  templates. `scripts/installer.py`: component planning and registration;
  `install_transaction.py`: ownership-safe durable rollback; `package.py`: local archives.
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
  commands that change state.** Installer tests use temporary HOME and all
  XDG/agent roots, stubbed manager commands and private D-Bus activation.
  Never run installation against the real desktop or interrupt its soak test.
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
  start an indexer, write the index or add a schema change. Its index API
  additions are `Reader::roots()` and `SearchOptions::accept`. The latter
  must stay `None` for daemon, KRunner and CLI searches. `accept`-only
  searches use an exact top-k early stop (name score + MAX_BONUS bound).
  Never truncate them by name score: depth and recency can reorder.
- Every path leaving the process passes `Policy::visible_under` *and* lies
  inside the requested scope. Client paths are validated, not normalized;
  never echo refused paths in errors.
- Disk metadata only via `beneath::stat_beneath` (no symlink traversal
  below the allowed dir); never `symlink_metadata` on client-visible paths.
  Enforce the policy via `accept` (inside the index budget), never only
  after the search. Never return the DB path or
  whole-index stats unless `expose_index_totals`. Deny matching is ASCII
  case-insensitive. (All from the GPT 6 Astra review, docs/MCP.md.)
- Structured outputs must satisfy their `outputSchema`: Claude Code
  rejects responses with missing `required` fields. Fields omitted by serde
  need `#[serde(default)]`; the protocol test checks this.
- Live agent checks use `claude -p --mcp-config … --strict-mcp-config` and
  `codex exec -c mcp_servers.…`, with temp synthetic data. Never register
  test servers in the user's agent configs.
- `tests/mcp-smoke.py` and `scripts/benchmark-mcp.py` (results in
  BENCHMARKS.md §10).

- Stale-prefix recovery filters deleted entries inside the candidate budget
  only after the normal query was starved. Preserve authorization before
  filesystem access. `omitted_stale` is a lower bound; concurrent retry
  exhaustion sets `incomplete_due_to_changes`. The 8300-deletion protocol
  regression covers scoped/unscoped queries and limits 1/10.

- MCP validation evidence: docs/benchmarks/mcp-validation-2026-10-31.
  Keep native filename comparisons semantic-equivalent and filesystem-oracle
  checked. wait4 peak RSS includes a pre-exec Python image; use separate
  post-exec sampling. Only index-costs.json supplies fresh-build costs.
  The scoped early-stop experiment was rejected (no demonstrated gain).
  Never run maintenance harnesses on a desktop data directory; they use
  disposable corpora and a private D-Bus session.

## Modular installer checkpoint

- Installer needs Python 3.11+; shell entrypoints only dispatch. No engine,
  schema, KRunner protocol or MCP server changes are needed for packaging.
- Default is interactive; explicit component flags are scriptable. Core is
  required by integrations; additions preserve previously installed components.
- Never restart on upgrade. --enable and uninstall --stop are explicit actions.
  Agent edits require a displayed plan, explicit roots and a separate yes.
- Ownership is hash-based. Never adopt conflicting legacy/modified files by
  name alone. Journal recovery refuses externally changed destinations.
- D-Bus argv has two escaping layers. Both activation paths use fixed shell
  code with the executable as a positional argument, never interpolated code
  (env alone mistakes '=' in a command path for an assignment). systemd uses
  ':' to disable dollar expansion. Keep real private-bus
  activation and systemd-analyze checks in tests/install-smoke.py.
- tests/package-smoke.py checks every component archive without Cargo/network.
  No release is published; signed artifacts/libc baseline remain future work.

## MCP access modes

- all_indexed is the default for new unconfigured MCP servers; legacy TOML
  allowlists without an explicit mode and CLI --allow arguments retain
  restricted semantics. An existing unconfigured MCP client is a scope-changing
  upgrade and must be clearly documented. all_indexed uses / as an
  authorization boundary ONLY; results still come from Reader search/lookup.
  home uses process HOME, never engine root mutation.
- Custom deny always wins over deny_exceptions; exceptions only relax built-in
  name rules. Hidden filtering and no-symlink metadata traversal remain active.
- Installer global registration requires typing all_indexed, then client-edit
  consent. Never change the live daemon or existing agent modes during tests.

## MCP discovery evaluation

- Metadata guidance fits within 512 initialization characters; keep the server
  name and tool names stable. Search descriptions must distinguish filename/path
  search from contents and authorization from indexed coverage.
- docs/MCP_DISCOVERY.md records actual fresh-session outcomes, including failures.
  Metadata alone did not improve primary spontaneous selection; do not claim it did.
- scripts/benchmark-mcp-discovery.py makes real authenticated model requests only
  when requested. Profiles/indexes are temporary; raw logs belong in bench-out.
  tests/mcp-discovery-smoke.py is the offline, credential-free CI counterpart.
