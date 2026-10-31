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
- `data/`: KRunner `.desktop` file, D-Bus `.service` file, systemd unit
  templates (`@BINDIR@`). `scripts/`: install and uninstall.
- `prototype/backend-bench`: the throwaway backend comparison. Not in the
  workspace; keep it as evidence.

## Commands

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release && tests/dbus-smoke.sh target/release/ferroseekd
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
  `scripts/uninstall.sh --purge`.
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
