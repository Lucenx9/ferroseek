# Launcher startup breakdown — 2026-11-13

This diagnostic analyzes existing successful graphical CI samples. It does not
measure a speedup from the subsequent view-handler refactoring.

Source: `a8523c9c0498935d95fdcc90837d050c27c6797d`, merged unchanged into
`e2734ab142169792b7ef7cd9ebd74831d20b7262`.
[Workflow run 43466788684](https://github.com/Lucenx9/ferroseek/actions/runs/43466788684),
job `132027938249`, artifact `launcher-interaction-us-1x` (`14059067909`).
The artifact's published SHA-256 is
`e0bd1f28a1773a7af5daf915915b085d4d260d7c3bb59aee943ec9ea19ab2135`;
the measured launcher binary SHA-256 is
`f58be49198020bff820907daac3e4dc5453783288e43d7d0577842c9ad9aa221`.

## Environment and method

Ubuntu 24.04 hosted runner, AMD EPYC 9V45 (four virtual CPUs), Linux
7.0.0-1017-azure, Rust 1.100.0 (`0ef1bc386`, from the job log), locked release
build. KWin 5.27.11 used llvmpipe (LLVM 20.1.2), Mesa 25.2.8. The US keyboard
profile had one 1280×800 logical output at scale 1. The launcher recorded WGPU
in auto/opaque mode and tiny-skia in software mode. `LIBGL_ALWAYS_SOFTWARE=1`;
WGPU backend/power and iced present-mode overrides were unset.

The existing isolated harness uses temporary HOME/XDG paths, a private bus with
empty activation directories, synthetic application entries and 10,007 indexed
files. It runs the launcher with `--private`. Modes rotate across 60 processes:
20 per mode, with the first in each mode kept separate from its 19 repeats.
The daemon/compositor stay alive; caches are not dropped. This is one hosted
runner job, not independent machine repetitions or physical desktop validation.

The complete original report passed `scripts/verify-launcher-report.py`, checking
60 samples against process logs, result generations, renderers, material and
input-readiness evidence. [summarize.py](summarize.py) then extracted the first
occurrence of five startup milestones into [samples.csv](samples.csv). Fields
are microseconds since the timing initializer at the start of `main`;
`first_in_mode` identifies the separately reported first process.

| Mode | First UI entry / redraw / focus (ms) | Repeat UI entry p50 (ms) | Repeat join interval p50 / max (ms) | Repeat redraw p50 / max (ms) | Repeat focus p50 / max (ms) |
|---|---:|---:|---:|---:|---:|
| Auto / WGPU | 1.972 / 690.235 / 877.273 | 0.793 | 0.022 / 0.025 | 438.191 / 468.881 | 467.853 / 498.986 |
| Opaque / WGPU | 0.806 / 434.484 / 478.493 | 0.780 | 0.021 / 0.033 | 436.647 / 476.451 | 466.752 / 506.541 |
| Software / tiny-skia | 0.749 / 190.583 / 220.098 | 0.780 | 0.020 / 0.024 | 191.987 / 195.472 | 197.006 / 200.022 |

p50 uses nearest rank (the tenth of 19 repeat values); at this sample count,
nearest-rank p95 equals maximum. Compute the join interval per process as
`applications_ready - backend_ready`, not by subtracting aggregate medians.

## Interpretation and decision

The repeat join interval is at most 0.033 ms here. Application discovery runs
concurrently with bus setup; this interval measures only the remaining join
wait and diagnostic overhead, not the scan's full duration. The synthetic
catalog is small, so these results cannot exclude a discovery bottleneck on a
real, larger catalog or slow storage. They do not justify delaying application
results, introducing a cache or changing startup concurrency.

Most observed startup time is between `ui_starting` and the first redraw/focus
milestones. That interval includes graphical runtime initialization and event
scheduling; these logs do not attribute its internal costs. Profile those stages
on the same environment before proposing renderer or lifecycle changes. Do not
change the default renderer from these software-rendered measurements alone.

`first_frame` is a redraw callback timestamp, not completed drawing or display
presentation. Keyboard focus is a compositor notification, not proof that the
search widget is ready at that instant. Process creation before `main`, global
shortcut dispatch, GPU drivers and physical display latency are excluded.
Diagnostics add overhead. No end-to-end physical latency or improvement over
another revision is claimed, and this is not a universal CI performance budget.

## Reproduce

Download the named artifact outside the worktree, then run from the repository
root (GitHub artifacts have a retention limit):

```sh
gh run download 43466788684 --repo Lucenx9/ferroseek \
  --name launcher-interaction-us-1x --dir /absolute/path/startup-artifact
python3 scripts/verify-launcher-report.py /absolute/path/startup-artifact
python3 docs/benchmarks/launcher-startup-2026-11-13/summarize.py \
  /absolute/path/startup-artifact > /absolute/path/startup.csv
cmp docs/benchmarks/launcher-startup-2026-11-13/samples.csv /absolute/path/startup.csv
```

For a new measurement use the existing isolated
[interaction harness](../../BENCHMARKS.md#12-launcher-interaction-latency), with
20 opens per mode and the same profile. Verify its full report before extracting
startup samples. Record source, binary hash, toolchain, renderer and environment;
keep the first process separate and do not interpret different machines as a
before/after comparison. The committed CSV preserves all startup observations
even after the original workflow artifact expires.
