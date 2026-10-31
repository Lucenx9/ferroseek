# Modular installer validation and review

Baseline: `0b7abbf`; implementation milestone: `10a6aaa`. Follow-up fixes were
made before pushing. Scope is installer/distribution code, tests and docs;
no core engine, CLI, daemon, KRunner protocol, MCP server or index schema edits.

## Standards

Two concrete findings against the repository's isolation and recovery rules:

1. Installer tests initially inherited `FERROSEEK_CONFIG` and
   `FERROSEEK_DATA_DIR`. Those take precedence over temporary HOME/XDG and could
   point a test daemon at live data. Tests now set both explicitly and isolate
   XDG runtime and agent directories. The initial test environment had neither
   override set. Tests also validate a private daemon's executable before
   assigning a PID eligible for cleanup.
2. Recovery validated destinations globally but did not recheck the expected
   replacement signature immediately before each restoration. It now rechecks
   every file and retains the journal on conflict. A fault-injection regression
   modifies an earlier destination during restoration of a later one and proves
   that the external edit is retained. This reduces the conflict window; it is
   not an atomic filesystem compare-and-swap against uncooperative writers.

Both findings are fixed. No additional structural-smell change was warranted.

## Spec

Three concrete findings (one already fixed during review):

1. Permission-only updates were not rolled back when old/new bytes matched.
   Journal signatures now include modes as well as content hashes. A regression
   verifies restoration of the original mode after a later injected failure.
2. Uninstallation originally inspected only systemd state. A direct D-Bus
   daemon could survive successful removal. Uninstall now checks NameHasOwner
   without activation and verifies the PID against the active unit. Unmanaged
   owners require manual stopping; actual private-bus activation exercises this
   refusal. Managed active/enabled units still require explicit `--stop`.
3. Dry-run omitted creation of initial configuration passed with `--config`.
   It now displays the destination and preserve/create decision, installer
   state, planned registration refreshes and explicit enable/stop actions.

All three findings are fixed. Differing legacy files require manual migration;
this is an intentional ownership-safety limit, documented in INSTALLATION.md.

## Validation

- 108 Rust tests passed; workspace formatting, Clippy with warnings denied,
  release builds and minimum-Rust 1.89 all-target checks passed locally.
- `python3 tests/install-smoke.py`: interactive/default and explicit component
  selection, headless/no-manager behavior, config/index preservation, additive
  upgrades, partial/repeated uninstall, enable/stop opt-ins, unrelated/modified
  file refusal, symlinks, unusual paths, agent consent/idempotence, injected
  write failure, mode rollback, abrupt-process recovery and external conflicts.
- Actual private D-Bus activation and `systemd-analyze --user verify` passed
  with spaces, Unicode, quotes, backslash, dollar, percent and shell punctuation
  in HOME. Actual user-systemd start/stop was not exercised: the running desktop
  service and soak test were intentionally preserved.
- The installed Claude Code and Codex CLIs successfully read generated entries
  in temporary user configurations (`FERROSEEK_TEST_AGENT_PARSERS=1`). This
  optional local check needs the clients; CI checks the config structures and
  preservation without requiring either CLI or an account.
- Tests also passed with inherited FerroSeek overrides deliberately pointing
  at a nonexistent guard path, confirming those values were replaced.
- `python3 tests/package-smoke.py`: all four native component archives,
  identical local rebuild bytes, normalized metadata, external/per-file
  checksums, no-clobber outputs, dry-run without new bytecode/files, unavailable
  component refusal, corrupt payload refusal, install without Cargo and removal.
- Existing benchmark, filesystem-oracle, MCP protocol (four versions) and
  private D-Bus smoke tests passed. New packaging tests are part of GitHub CI.

Installer tests never install into the real HOME. The desktop daemon retained
its original PID/start time and the 72-hour soak timer remained active. No
release or archive upload was performed. Remaining binary-publication work is
listed in [INSTALLATION.md](INSTALLATION.md#local-distribution-archives-no-release-published).
