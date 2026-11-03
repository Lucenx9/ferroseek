# Current launcher renderer diagnostics

Source `d5559f381015a2aa45c06cd36235c1030aeebeae`, release launcher built with
Rust 1.94.0, locked dependencies and the workspace release profile. Binary SHA-256:
`bcd247bf3f1b472d2d1736e62cf8bca33fb08b642e9143c44c7f69dee25e6698`.

The run used an Ubuntu 24.04 container, glibc 2.39, Python 3.12.3 and virtual
KWin 5.27.11 on the same Intel N95 host, four CPUs and approximately 16 GiB RAM.
The host kernel and complete CPU description are retained in each report.
KWin used Mesa llvmpipe. The launcher requested Vulkan with the lavapipe manifest
`/usr/share/vulkan/icd.d/lvp_icd.json`; no GPU devices were passed into the container.
WGPU/tiny-skia identity was observed separately in each process, not inferred
from a screenshot or the requested mode. The host was not a dedicated benchmark
machine; no compilation ran during these samples.

## Completed checks

All three profiles completed 20 opens per mode, 60 per profile and 180 in total.
Each profile used its own private bus, daemon, temporary HOME/XDG tree and
10,007-file corpus. Within a profile, modes rotated on the same compositor and
warm daemon. Actual output geometry, scale and keyboard symbols were checked.
Application/document activation, keyboard/pointer actions, focus return, blur,
opaque/software controls and preview expansion/restoration passed. The separate
screenshot-only smoke mode passed too.

The report verifier reconciled every sample with its per-process snapshot stream,
original process log and aggregates, and checked the executable hash and material resize evidence.
A deliberate material-capture failure retained six samples as incomplete;
the verifier refused that report. An injected failure on the third launcher
open retained the first two memory samples without a completed report.
Altered intervals, backend labels and
percentiles were rejected in separate checks. The source tree's 353 Rust tests,
format check, Python/shell syntax checks and Clippy 1.99.0 also passed locally.

## US 1x diagnostic snapshot

Times are milliseconds. Repeats exclude the first process of each mode; n=19,
so nearest-rank p95 equals maximum. RSS is the launcher alone, in MiB, sampled
after results and before more input.

| Mode / observed renderer | First process redraw | Repeat redraw p50 | Repeat focus p50 | Repeat query redraw p50 / p95 | Repeat RSS p50 |
|---|---:|---:|---:|---:|---:|
| Auto / WGPU | 332.723 | 336.946 | 377.133 | 35.017 / 56.996 | 126.32 |
| Opaque / WGPU | 321.960 | 333.098 | 372.012 | 16.387 / 32.185 | 126.20 |
| Software / tiny-skia | 306.897 | 266.566 | 276.120 | 33.735 / 47.499 | 17.80 |

These are redraw callbacks and focus notifications, not completed rendering or
physical shortcut-to-display latency. RSS includes shared Mesa/LLVM pages,
excludes GPU memory and omits the daemon. HWM is a snapshot high-water through
that read. No caches were dropped; first-process and repeated samples are not
cold-cache claims. The other profiles are separate datasets, not evidence that
one keyboard layout or scale is faster. Hardware GPU drivers and physical
multiple-monitor desktops remain unvalidated. No performance improvement over
the November 2 measurements is claimed.

## Startup stability caveat

A preliminary 1.5x run timed out before redraw on its thirteenth open; an earlier
preliminary run also stalled before an activation launch drew. Those attempts
were not counted as completed datasets. After adding backend/application/UI
startup milestones, a reduced 300-open startup-only stress run completed, as
did the accepted profiles below. No startup root cause or timeout fix is claimed.
The milestone events and per-process sample stream preserve evidence for a
future recurrence. This remains a stability gap for extended desktop validation.

## Resize finding

A requested-size blur update could precede the configured viewport. The original
Vulkan full run failed its unchanged expanded-preview pixel assertion twice;
a size-observation-only probe also failed. Two subsequent captures after one
failure showed the correct image. The fix keeps fresh legacy blur objects and
updates their regions from actual iced open/resize events. Five fast Vulkan
material cases then passed, followed by the three complete profiles above.

The harness waits for the new observed width and still checks the original
pixels. This synchronizes the fixture; it does not certify every intermediate
frame or physical compositor presentation.

Both images below are native captures from an isolated KWin session with
synthetic files and a striped backdrop.

[Before the ordering fix](preview-before.png) · [After the ordering fix](preview-after.png)

## Data and reproduction

- [US 1x samples and environment](us-1x.json), [material controls](us-1x-material.json).
- [French AZERTY 1x samples and environment](fr-1x.json), [material controls](fr-1x-material.json).
- [US 1.5x samples and environment](us-1.5x.json), [material controls](us-1.5x-material.json).

Use the dependencies in `.github/workflows/launcher-interaction.yml` on Ubuntu
24.04. In the test container only, remove the packaged KWin file capability
with `setcap -r /usr/bin/kwin_wayland` so the unprivileged container can execute
it. The repository was mounted read-only and only the output tree was writable.
The image itself is a local test artifact, not a published distribution.

```sh
cargo build --release --locked -p ferroseek-launcher -p ferroseek-daemon
launcher_out=$(mktemp -d)
LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=vulkan \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json \
  FERROSEEK_SMOKE_INTERACTION=1 FERROSEEK_SMOKE_MODE=all \
  FERROSEEK_SMOKE_PROFILE=us-1x FERROSEEK_SMOKE_TRIALS=20 \
  xvfb-run -a tests/launcher-smoke.sh target/release "$launcher_out"
python3 scripts/verify-launcher-report.py "$launcher_out" \
  --binary target/release/ferroseek-launcher
```

Repeat with fresh output directories for `fr-1x` and `us-1.5x`. The committed
JSON files are byte-for-byte report copies. Original per-process logs and PNGs
remain in the local `target/launcher-container/final-v4` tree; graphical CI
uploads equivalent complete evidence for each new run. To rerun the verifier,
keep a report with the original logs, not only the committed JSON copy.
