# FerroSeek: notes for agents

FerroSeek is a keyboard-first Linux launcher for applications, files and
calculations, powered by its Rust filename/path search engine. Prioritize daily
launcher reliability. Keep the engine/CLI independently usable and MCP optional,
sharing one index. Noctalia/KRunner adapters are removed; historical research
is in `docs/RESEARCH.md`.

Read [DESIGN.md](DESIGN.md) at the start of every task. It is the design contract
for UI, interactions, user-facing copy and product presentation. Apply its rules
and review checklist to affected work; preserve its product constraints in
engine-only tasks without expanding their scope. Update the guide in the same
change when intentionally changing a design rule or baseline.

## Change discipline

- Solve the current user-visible problem with the smallest coherent change.
  Reuse modules and patterns; add abstractions only for current callers' needs.
- Prefer existing files and test suites. Add a file or dependency only when
  it has a distinct responsibility that the existing code cannot reasonably hold.
- Handle failures at real system boundaries. Preserve recovery and security
  constraints; add fallback paths or configuration only for a demonstrated need.
- Keep shared behavior in its owning module. CLI, D-Bus, launcher and MCP
  adapters translate requests and responses without duplicating domain logic.
- Check affected entry points: keyboard shortcuts, menus and pointer actions;
  launcher, CLI, D-Bus and MCP where applicable. Preserve intentional access
  differences. Include the way to undo or leave each reversible state.
- Verify descriptive guidance against current code and configuration. Report
  discrepancies and update obsolete text; preserve explicit product, recovery
  and security constraints rather than treating an implementation bug as policy.
- Write concise English. Comments explain constraints or reasons. Document
  decisions and traps the code cannot explain; link to code for implementation.
  Update affected guidance in place and distinguish available features from plans.
- Keep temporary plans, research scratch files and PR-only evidence outside the
  worktree. Preserve committed benchmarks, reproducible evidence and guides.
- Keep this file below 200 lines. Put specialist constraints in the guides below
  and replace obsolete guidance instead of appending per-task checkpoints.

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
Update inaccurate screenshots and affected setup/limits after code or dependency changes; link architecture, benchmarks and historical research from detailed guides.

## Read before changing a component

- Index/engine/daemon: read `docs/ARCHITECTURE.md`, including Contributor
  constraints, and `docs/CODE_REVIEW.md`. Only `crates/index` knows SQL; only
  the engine's `reconcile_dir` and `resolve` reconcile filesystem state.
- Launcher/shared config: read `docs/LAUNCHER.md`, including Contributor
  constraints, Tests and Desktop validation. The launcher never opens the index;
  file results and opens go through the daemon's `Search`/`Open`.
- MCP: read `docs/MCP.md`, including Contributor constraints. MCP stays read-only,
  stdio-only and opt-in. Discovery, tool-description or schema changes also require
  `docs/MCP_DISCOVERY.md` and `python3 tests/mcp-discovery-smoke.py`. Model requests
  run only when requested, with temporary profiles and synthetic data.
- Installer/packaging: read `docs/INSTALLATION.md`, including Contributor
  constraints. Use `python3 tests/install-smoke.py` and
  `python3 tests/package-smoke.py`; both isolate installation from the desktop.
- Measurements: read `docs/BENCHMARKS.md`; preserve `prototype/backend-bench`.

## Verification

- Test changed behavior and plausible regressions at the lowest layer that
  reproduces the failure. Extend an existing case where it fits; avoid repeating
  the same case in wrappers or testing guarantees already supplied by types.
- For logic, state, data and protocol bugs, reproduce the failure with a focused
  regression test before fixing it. Purely visual fixes use before/after captures;
  prose and formatting changes need document checks rather than behavior tests.
- Async tests await a response, channel message or explicit completion signal.
  Use bounded deadlines for hangs; arbitrary sleeps or retries are not proof.
- Run relevant checks first, then workspace checks for shared-code changes or CI.
  Stop repeating passing checks unless code changes or new evidence requires it.
- Keep mechanically checkable invariants in tests or CI. Report the checks run,
  their results and any verification that remains incomplete.

```sh
cargo fmt --all --check
cargo test -p ferroseek-launcher --locked  # replace the package for other modules
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo +1.89.0 check --workspace --all-targets --locked
cargo build --release --workspace --locked
tests/dbus-smoke.sh target/release/ferroseekd
python3 tests/mcp-smoke.py target/release/ferroseek-mcp target/release/ferroseek
tests/launcher-smoke.sh target/release target/launcher-smoke
```

Rust minimum is declared in `Cargo.toml`; CI toolchains and checks are in
`.github/workflows/ci.yml`. CI runs Clippy on `stable`; use that toolchain
locally too. If the default differs, run `cargo +stable clippy` with the flags
above instead of pinning a historical version.

## Safety and shared invariants

- Tests never touch the real home directory, index or session bus.
  Use `tempfile` and `Paths { .. }`, or `FERROSEEK_CONFIG`/`FERROSEEK_DATA_DIR`.
  D-Bus tests use `dbus-run-session`; smoke scripts/bench re-exec themselves.
  CLI tests set `DBUS_SESSION_BUS_ADDRESS` to a dead socket.
- Never modify KDE or Baloo configuration or run state-changing `balooctl`
  commands. Installer tests isolate HOME and all XDG/agent roots, use stubbed
  manager commands and private D-Bus activation. Never install against the real
  desktop or interrupt its soak test; never commit real user indexes/configs/logs.
- Track the PID of each daemon or compositor you start and stop only those
  processes. Never use `pkill -f` or terminate processes by matching names or paths;
  other worktrees and the developer's live session may use the same binaries.
- Index changes are state-based. Events mark `(dir_id, name)` dirty; `resolve()`
  checks current filesystem state. Never delete on `ENOENT` unless the parent
  still has its stored inode. Directory `mtime` means "listing reconciled at";
  only `Txn::mark_dir_reconciled` sets it. Preserve incomplete listings on errors.
- Performance claims need a measurement in `docs/BENCHMARKS.md`. Keep the
  environment and commands current. Baloo comparisons require its section 6.
- zbus is v6: check sources in `~/.cargo/registry`. Features are `default-rt`,
  `proxy`, `service`; there is no blocking module. `zbus::block_on` drives sync
  code; `zruntime::unblock` runs blocking work.
- Commits end with the `Co-Authored-By` trailer in use in `git log`.
