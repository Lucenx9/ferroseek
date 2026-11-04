# Launcher input readiness diagnostics

Source `d9474b0fd8ece0813dfe698e2310ca0d6e55c74f`, release build with Rust
1.99.1 and locked dependencies. [Completed workflow](https://github.com/Lucenx9/ferroseek/actions/runs/41824622770)
ran on Ubuntu 24.04 GitHub-hosted runners, AMD EPYC 7763 with four logical CPUs,
glibc 2.39 and virtual KWin 5.27.11. KWin reported Mesa llvmpipe (LLVM 20.1.2).
The launcher observed WGPU for auto/opaque and tiny-skia for software. WGPU's
adapter was not recorded; no hardware GPU performance claim is made. Full CPU,
kernel, graphics environment and executable SHA-256 are in each report.

## Completed checks

Three profiles each completed 20 opens per mode: 180 measured opens total.
Modes rotated on each profile's warm compositor and daemon with 10,007 synthetic
files and isolated HOME/XDG/session bus. Profiles ran on separate runners.
Keyboard symbols, geometry and scale were checked. Existing activation, pointer,
preview/material, focus-return and screenshot-only checks passed.

Nine additional processes cover input bursts, one per profile/mode. Typing starts
after compositor keyboard Enter, without a redraw or widget-focus readiness gate.
Ctrl+K plus immediate text, close/reopen/edit sequences, Escape clear/close and a
context click plus immediate text run without per-key roundtrips. Each process
records 10 main-query edits, 13 action-filter edits, one filter clear and two
menu closes. Real file-result generations 6 and 10 must each contain 15 files.
The persistent-widget tests separately cover one event batch, initial `--query`,
native select-all/Unicode, stale focus operations and deferred input cancellation
on focus loss. The launcher suite passed all 189 tests.

## US 1x diagnostic snapshot

Times are milliseconds except RSS (MiB). Repeats exclude the first process of
each mode; n=19, so nearest-rank p95 equals maximum. Per-stage percentiles are
independent. RSS is sampled with results visible, before further input.

| Mode / observed renderer | First process redraw | Repeat redraw p50 | Repeat focus p50 | Repeat query redraw p50 / p95 | Repeat RSS p50 |
|---|---:|---:|---:|---:|---:|
| auto/wgpu | 1345.827 | 681.679 | 723.692 | 28.858 / 30.786 | 161.66 |
| opaque/wgpu | 682.659 | 684.040 | 725.540 | 14.133 / 25.177 | 161.72 |
| software/tiny-skia | 305.600 | 305.157 | 310.877 | 22.442 / 43.417 | 18.35 |

These observations refresh the baseline; they do not establish a speed gain.
The previous renderer dataset used different hardware and Rust. No caches were
dropped; first process does not mean cold cache. Redraw callbacks, compositor
keyboard Enter and in-tree widget focus are separate observations, not completed
presentation or physical shortcut-to-display latency. In-tree timestamps are
captured before forwarding input; log delivery may occur later. RSS includes
shared driver pages, excludes GPU memory and omits the daemon. The profiles are
separate datasets, not layout/scale performance comparisons. Physical desktops,
IME and keystrokes before compositor focus remain unverified.

## Data and reproduction

- [US 1x samples/environment](us-1x.json), [burst observations](us-1x-input.json).
- [AZERTY 1x samples/environment](fr-1x.json), [burst observations](fr-1x-input.json).
- [US 1.5x samples/environment](us-1.5x.json), [burst observations](us-1.5x-input.json).

These files are byte-for-byte copies from workflow artifacts named
`launcher-interaction-{profile}`. Download the full artifacts to rerun the
verifier: they retain original process logs, `samples.jsonl`, material evidence
and PNGs. The committed JSON copies alone are insufficient. Every report was
verified against its original logs by CI and after download.

Use `.github/workflows/launcher-interaction.yml` on Ubuntu 24.04, or its Linux
dependencies and the following command with a new output directory per profile:

```sh
cargo build --release --locked -p ferroseek-launcher -p ferroseek-daemon
launcher_out=$(mktemp -d)
LIBGL_ALWAYS_SOFTWARE=1 FERROSEEK_SMOKE_INTERACTION=1 \
  FERROSEEK_SMOKE_MODE=all FERROSEEK_SMOKE_PROFILE=us-1x \
  FERROSEEK_SMOKE_TRIALS=20 \
  xvfb-run -a tests/launcher-smoke.sh target/release "$launcher_out"
python3 scripts/verify-launcher-report.py "$launcher_out" \
  --binary target/release/ferroseek-launcher
```

Repeat with `fr-1x` and `us-1.5x`. Test-only input is enabled only in disposable
virtual KWin. These checks do not operate the real desktop or user index.
