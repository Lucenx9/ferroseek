# FerroSeek: notes for agents

FerroSeek is a keyboard-first Linux launcher for applications, files and
calculations, powered by its Rust filename/path search engine. Prioritize daily
launcher reliability. Keep the engine/CLI independently usable and MCP optional,
sharing one index. Noctalia/KRunner adapters are removed; historical research
is in `docs/RESEARCH.md`.

## Change discipline

- Solve the current user-visible problem with the smallest coherent change.
  Reuse modules and patterns; add abstractions only for current callers' needs.
- Prefer existing files and test suites. Add a file or dependency only when
  it has a distinct responsibility that the existing code cannot reasonably hold.
- Handle failures at real system boundaries. Preserve the recovery and security
  rules below; add fallback paths or configuration only for a demonstrated need.
- Test changed behavior and plausible regressions. Extend an existing test when
  possible; skip tests that restate helpers or only cover prose/formatting edits.
- Run relevant checks first, then workspace checks for shared-code changes or CI.
  Stop repeating passing checks unless code changes or new evidence requires it.
- Write concise English. Comments explain constraints or reasons; documentation
  describes available behavior and its limits. Update only affected instructions.
- Keep this file below 200 lines, with current constraints and links to existing
  guides. Replace obsolete guidance instead of appending per-task checkpoints.

## Product, UX and README direction

- Build a fast, keyboard-first launcher with contextual actions and composable commands. Keep common tasks direct and reveal advanced choices when needed.
- Prioritize reliable focus/activation, measured opening-to-results latency, clear initial setup and complete daily workflows before adding more tools or an extension platform; follow `docs/ROADMAP.md`.
- Keep Enter, Escape, navigation, filtering and numbered actions consistent across views. Preserve the selected target and action identities through asynchronous updates; make essential actions keyboard-accessible.
- Help users distinguish similar results with readable names, paths and relevant details. Add previews or extra controls only when they help choose or act on a result.
- Preserve FerroSeek's restrained visual language: clear typography and spacing, neutral surfaces, a limited accent and legible selection/focus states. Extend existing components and tokens before inventing another style.
- Show loading, empty, success and error states with plain language and a useful next step. Make settings discoverable in the relevant flow; keep implementation details out of normal interactions.
- For interaction changes, verify the complete affected flow, keyboard and pointer behavior, focus, long labels and asynchronous/error cases. Inspect real rendered UI for layout changes; distinguish virtual compositor checks from real desktop validation and measure latency before making speed claims.
- Use original implementations and platform conventions. Keep competitor names, comparisons, reverse-engineering artifacts and proprietary code/assets out of product changes, documentation, commits and PR descriptions.

Keep the README centered on the Linux launcher: short description and real screenshot, implemented tasks, requirements/install, starting and binding a shortcut, keys, current limits/next steps, then optional engine/CLI/MCP tools.
Write concise English and concrete actions; separate available features from plans. Update inaccurate screenshots and affected setup/limits after code or dependency changes; link architecture, benchmarks and historical research from detailed guides.

## Map

- For index/engine/daemon changes, read `docs/ARCHITECTURE.md` and
  `docs/CODE_REVIEW.md`. Only `crates/index` knows SQL; only the engine's
  `reconcile_dir` and `resolve` reconcile filesystem state with the index.
- For launcher/shared-config changes, read `docs/LAUNCHER.md`.
- For MCP changes, read `docs/MCP.md`; for installer/packaging changes,
  read `docs/INSTALLATION.md` and use the existing Python smoke tests.
- For measurements, read `docs/BENCHMARKS.md`; preserve `prototype/backend-bench`.

## Commands

```sh
cargo fmt --all --check
cargo test -p ferroseek-launcher --locked  # replace the package for other modules
cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo +1.89.0 check --workspace --all-targets --locked
cargo build --release --workspace --locked
tests/dbus-smoke.sh target/release/ferroseekd
```

Rust minimum is 1.89. CI stable was 1.99; use that toolchain for Clippy
rather than only the local default (1.94). Graphical checks use
`tests/launcher-smoke.sh target/release target/launcher-smoke`.

## Rules

- **Tests never touch the real home directory, index or session bus.**
  - Use `tempfile` and `Paths { .. }`, or `FERROSEEK_CONFIG`/`FERROSEEK_DATA_DIR`.
  - D-Bus tests use `dbus-run-session`; smoke scripts/bench re-exec themselves.
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
- **zbus is v6.** Check sources in `~/.cargo/registry`: features are
  `default-rt`, `proxy`, `service`; there is no blocking module.
  `zbus::block_on` drives sync code; `zruntime::unblock` runs blocking work.
- `gen` is a reserved keyword in edition 2024.
- Commits end with the `Co-Authored-By` trailer in use in `git log`.

## Index and daemon constraints

- Policy changes invalidate directory listings transactionally through
  `Writer::apply_policy`; preserve crash-safe invalidation on future changes.
- Inotify reads must return after one buffer so channel backpressure works.
- Search budgets count candidates *after* filters. Path-only queries stream
  entries; do not infer a mandatory filename token from a directory path.
- Path searches derive bounded directory scopes from existing name FTS and
  parent indexes; no schema change. Root/broad anchors fall back; short path
  components may still scan. Preserve explicit prefix ordering before caps.
- All search stages share one read transaction. Tests compare independent
  path lists and exercise concurrent directory renames.
- Reader-pool epoch and database identity checks protect Release/recovery.
  Never restore per-Status malloc_trim or blocking async control work.
- Benchmark --work must be new/empty unless explicitly reusing a matching
  generated corpus. Reuse initial indexing deletes only that work/data.
- Incomplete listings and failed child stats must never imply deletion.
  Preserve existing entries, count errors and invalidate directory listing
  timestamps so future quick passes retry. Event stat errors enter bounded
  retry handling. Overflow and dropped-event recovery must use Full mode:
  changing file contents does not update its parent directory's mtime.
- `scripts/verify-index.py` is an independent read-only desktop oracle. Use
  complete effective config; reports contain counts, not private file paths.
  See docs/DESKTOP_SOAK.md. Never commit real user indexes/configs/logs.
- Benchmark library rows use the harness library even with an old daemon;
  compare D-Bus rows only. Legacy relevance percentages counted skipped samples
  as successes; use the corrected denominator and measured workload in the guide.

## MCP constraints

- `ferroseek-mcp` must stay read-only, stdio-only and opt-in. It must never
  start an indexer, write the index or add a schema change. Its index API
  additions are `Reader::roots()` and `SearchOptions::accept`. The latter
  must stay `None` for daemon and CLI searches. `accept`-only
  searches use an exact top-k early stop (name score + MAX_BONUS bound).
  Never truncate them by name score: depth and recency can reorder.
- Every path leaving the process passes `Policy::visible_under` *and* lies
  inside the requested scope. Client paths are validated, not normalized;
  never echo refused paths in errors.
- Disk metadata only via `beneath::stat_beneath` (no symlink traversal
  below the allowed dir); never `symlink_metadata` on client-visible paths.
  Enforce policy via `accept` inside the index budget, never only after search.
  Never return the DB path or whole-index stats unless `expose_index_totals`.
  Deny matching is ASCII case-insensitive.
- Structured outputs must satisfy `outputSchema`, including `required` fields.
  Fields omitted by serde need `#[serde(default)]`; the protocol test checks this.
- Live agent checks use `claude -p --mcp-config … --strict-mcp-config` and
  `codex exec -c mcp_servers.…`, with temp synthetic data. Never register
  test servers in the user's agent configs.
- Stale-prefix recovery filters deleted entries inside the candidate budget
  only after the normal query was starved. Preserve authorization before
  filesystem access. `omitted_stale` is a lower bound; concurrent retry
  exhaustion sets `incomplete_due_to_changes`.
- MCP validation is in `docs/benchmarks/mcp-validation-2026-10-31/`. Keep native
  comparisons semantic-equivalent and filesystem-oracle checked. wait4 peak RSS
  includes a pre-exec Python image; use post-exec sampling. Only index-costs.json
  supplies fresh-build costs. Harnesses use disposable data and a private bus.
- all_indexed is the default for new unconfigured MCP servers; legacy TOML
  allowlists without an explicit mode and CLI --allow retain restricted semantics.
  Document this scope-changing upgrade. all_indexed uses / only as an authorization
  boundary; results still come from Reader. home uses HOME, never engine roots.
- Custom deny wins over deny_exceptions; exceptions only relax built-in names.
  Preserve hidden filtering and no-symlink traversal. Global registration requires
  typing all_indexed, then client-edit consent; tests never alter live agent modes.
- For discovery changes, read `docs/MCP_DISCOVERY.md`. Metadata guidance fits
  within 512 initialization characters; keep server/tool names stable and filename
  search distinct from contents, authorization distinct from indexed coverage.
  Metadata alone did not improve spontaneous selection in the recorded evaluation.
- `scripts/benchmark-mcp-discovery.py` makes authenticated model requests only
  when requested; use temporary profiles/indexes and keep raw logs in bench-out.
  `tests/mcp-discovery-smoke.py` is the offline CI check.

## Installer constraints

- Installer needs Python 3.11+; shell entrypoints only dispatch. No engine,
  schema, search protocol or MCP server changes are needed for packaging.
- Default is interactive; on Wayland recommend launcher + core, otherwise core.
  `--launcher` installs the UI; package.py defaults to launcher archives.
  Explicit component flags are scriptable; preserve previously installed components.
- Never restart on upgrade. --enable and uninstall --stop are explicit actions.
  Agent edits require a displayed plan, explicit roots and a separate yes.
- Ownership is hash-based. Never adopt conflicting legacy/modified files by
  name alone. Journal recovery refuses externally changed destinations.
- D-Bus argv has two escaping layers. Both activation paths use fixed shell
  code with the executable as a positional argument, never interpolated code
  (env alone mistakes '=' in a command path for an assignment). systemd uses
  ':' to disable dollar expansion. Keep real private-bus
  activation and systemd-analyze checks in tests/install-smoke.py.
- `tests/package-smoke.py` checks archives without Cargo/network. No release
  is published; signed artifacts/libc baseline remain future work.

## Launcher constraints

- `ferroseek-launcher` stays pure Rust: no GTK/Qt or crates that build C;
  this is why `.desktop` parsing is local. Building needs libxkbcommon headers
  for sctk's pkg-config probe. Revisit the `winit-core` beta.2 pin for
  `iced_exdevtools` 0.19.1 when bumping iced_layershell.
- It never opens the index: files come from the daemon's `Search`/`Open`
  (D-Bus activation), so `Open` keeps refusing non-indexed paths.
- zbus stays on its own thread inside `zbus::block_on`; never await zbus
  futures on iced's executor. Replies carry the query generation.
- A new query clears old file rows; late results keep the selected target.
  Only one open is in flight; the process exits after it succeeds.
- History boosts reorder within a match class only (exact > strong > weak);
  history writes are locked read-modify-write, `0600`, honour `--private`.
- Graphical smoke tests use nested `kwin_wayland --virtual`, temp HOME/XDG and
  a private bus with an EMPTY activation dir to exclude user daemons and portals.
- No XDG activation token yet: iced_layershell 0.19 does not expose the
  layer surface (`window::run` is unhandled).
