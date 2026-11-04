#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Real keyboard/focus checks and diagnostic latency samples in virtual KWin."""
import ast
from dataclasses import asdict, dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import select
import signal
import struct
import subprocess
import sys
import time


@dataclass(frozen=True)
class LaunchMode:
    arguments: tuple[str, ...] = ()
    backend: str | None = None


MODES = {"auto": LaunchMode(), "opaque": LaunchMode(("--opaque",)),
         "software": LaunchMode(backend="tiny-skia")}


@dataclass(frozen=True)
class LauncherSample:
    trial: int
    mode: str
    renderer: str
    generation: int
    first_frame_us: int
    keyboard_focus_us: int
    query_ready_us: int
    ready_to_frame_us: int
    query_frame_us: int
    rss_snapshot_kib: int
    hwm_snapshot_kib: int


def launcher_environment(mode, **overrides):
    environment = {**os.environ, "FERROSEEK_LAUNCHER_TIMINGS": "1", **overrides}
    environment.pop("ICED_BACKEND", None)
    if MODES[mode].backend is not None:
        environment["ICED_BACKEND"] = MODES[mode].backend
    return environment


def observed_renderer(path):
    backends = {record["event"].removeprefix("renderer_") for record in records(path)
                if record.get("event") in {"renderer_wgpu", "renderer_tiny_skia"}}
    assert len(backends) <= 1, "a launcher changed renderer during its sample"
    return next(iter(backends), None)


def memory_snapshot(pid):
    fields = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines())
    return {key: int(fields[key].split()[0]) for key in ("VmRSS", "VmHWM")}


def probe(path, screen_name=None):
    from PyQt5.QtCore import QEvent
    from PyQt5.QtWidgets import QApplication, QLineEdit

    class Input(QLineEdit):
        def event(self, event):
            if event.type() == QEvent.FocusIn:
                with open(path, "a") as stream:
                    stream.write(json.dumps({"event": "focus"}) + "\n")
            return super().event(event)

    app = QApplication([])
    field = Input()
    field.setWindowTitle("FerroSeek isolated focus probe")
    field.textChanged.connect(lambda text: Path(path).write_text(json.dumps({"text": text}) + "\n"))
    if screen_name is None:
        field.showMaximized()  # Any point outside the launcher panel lands on a probe.
    else:
        field.winId()
        field.windowHandle().setScreen(next(screen for screen in app.screens() if screen.name() == screen_name))
        field.showFullScreen()
    field.setFocus()
    app.exec()


def material_backdrop(path):
    from PyQt5.QtCore import QEvent
    from PyQt5.QtGui import QColor, QPainter
    from PyQt5.QtWidgets import QApplication, QWidget

    class Backdrop(QWidget):
        def event(self, event):
            if event.type() == QEvent.WindowActivate:
                Path(path).write_text("focused")
            return super().event(event)

        def paintEvent(self, event):
            painter = QPainter(self)
            for x in range(0, self.width(), 2):
                value = 32 if (x // 2) % 2 else 224
                painter.fillRect(x, 0, 2, self.height(), QColor(value, value, value))

    app = QApplication([])
    window = Backdrop()
    window.setWindowTitle("FerroSeek isolated material backdrop")
    window.showFullScreen()
    app.exec()


def records(path):
    output = []
    if path.exists():
        for line in path.read_text(errors="replace").splitlines():
            try:
                output.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return output


def wait_for(predicate, description, timeout=15, process=None):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        if process is not None and process.poll() is not None:
            raise AssertionError(f"{description}: launcher exited with status {process.returncode}")
        time.sleep(0.01)
    raise AssertionError("Timed out: " + description)


def multimonitor(binary, out, helper):
    """Two real virtual outputs, changed only on the disposable KWin socket."""
    from PyQt5.QtGui import QImage
    assert os.environ.get("FERROSEEK_SMOKE_ISOLATED") == "1"
    assert os.environ.get("WAYLAND_DISPLAY", "").startswith("wayland-ferroseek-smoke-")
    control_env = {**os.environ, "KSCREEN_BACKEND_INPROCESS": "1", "KSCREEN_BACKEND": "KWayland"}
    processes = []
    configuration = Path(os.environ["FERROSEEK_CONFIG"])
    original = configuration.read_bytes()
    snapshots = []

    def doctor(*args):
        return subprocess.check_output(["kscreen-doctor", *args], env=control_env, text=True, timeout=15)

    def topology(label, expected):
        support = ast.literal_eval(subprocess.check_output([
            "gdbus", "call", "--session", "--dest", "org.kde.KWin", "--object-path", "/KWin",
            "--method", "org.kde.KWin.supportInformation"], text=True, timeout=5))[0]
        found = [[int(v) for v in entry] for entry in re.findall(
            r"(?m)^Geometry: (-?\d+),(-?\d+),(\d+)x(\d+)\s*$", support)]
        scales = [float(v) for v in re.findall(r"(?m)^Scale: ([0-9.]+)\s*$", support)]
        observed = sorted(zip(found, scales))
        if observed != sorted(expected):
            return None
        (out / f"topology-{label}.txt").write_text(support)
        snapshots.append({"stage": label, "outputs": observed})
        return observed

    def apply(label, expected, *args):
        (out / f"configure-{label}.log").write_text(doctor(*args))
        wait_for(lambda: topology(label, expected), "applied non-overlapping output geometry")

    def keys(*codes):
        subprocess.run([str(helper), *map(str, codes)], check=True, timeout=5)

    def chord(code):
        keys(29, code, -code, -29)

    alphabet = dict(zip("abcdefghijklmnopqrstuvwxyz", [
        30,48,46,32,18,33,34,35,23,36,37,38,50,49,24,25,16,19,31,20,22,47,17,45,21,44]))

    def type_text(value):
        for character in value:
            code = alphabet[character]
            keys(code, -code)

    def owner():
        return "true" in subprocess.check_output([
            "gdbus", "call", "--session", "--dest", "org.freedesktop.DBus", "--object-path", "/org/freedesktop/DBus",
            "--method", "org.freedesktop.DBus.NameHasOwner", "io.github.ferroseek.Launcher"], text=True, timeout=5)

    def capture(name, expected, scale):
        destination = out / f"{name}.png"
        with destination.with_suffix(".log").open("w") as stream:
            subprocess.run(["spectacle", "-b", "-n", "-m", "-o", str(destination)], check=True, timeout=15,
                           stdout=stream, stderr=stream,
                           env={**os.environ, "XDG_DATA_DIRS": "/usr/local/share:/usr/share", "XDG_CONFIG_DIRS": "/etc/xdg"})
        image = QImage(str(destination))
        assert (image.width(), image.height()) == (1280, 800), "capture must retain the selected output's native mode"
        def dark(x, y):
            color = image.pixelColor(x, y)
            return max(color.red(), color.green(), color.blue()) < 160
        left, top, width, height = [v * scale for v in expected]
        ys = [y for y in range(image.height()) if sum(dark(x, y) for x in range(0, image.width(), 4)) > width * 0.7 / 4]
        xs = [x for x in range(image.width()) if sum(dark(x, y) for y in range(round(top+30), round(top+height-30), 6)) > (height-60)*0.65/6]
        assert xs and ys, "launcher missing from selected monitor capture"
        actual = [min(xs), min(ys), max(xs)+1, max(ys)+1]
        wanted = [left, top, left+width, top+height]
        assert all(abs(a-b) <= 4 for a,b in zip(actual,wanted)), (name, actual, wanted)
        return actual

    def start_probe(index):
        path = out / f"probe-{index}.jsonl"
        path.unlink(missing_ok=True)
        p = subprocess.Popen([sys.executable, __file__, "--probe", str(path), names[index]])
        processes.append(p)
        wait_for(lambda: records(path), "fullscreen background input probe mapped", process=p)
        return p

    def focus(index):
        x = 200 if index == 0 else 1480
        subprocess.run([str(helper), "--double-click", str(x), "150"], check=True, timeout=5)
        chord(30)
        type_text("p" if index == 0 else "s")
        wanted = {"text": "p" if index == 0 else "s"}
        wait_for(lambda: wanted in records(out / f"probe-{index}.jsonl"), "input on selected output")

    def launch(mode, label, bounds):
        log = out / f"multi-{mode}-{label}.jsonl"
        with log.open("w") as stream:
            p = subprocess.Popen([str(binary), "--private", "--query", "report", *MODES[mode].arguments],
                                 stdout=stream, stderr=stream, env=launcher_environment(mode))
        processes.append(p)
        wait_for(lambda: any(r.get("event") == "keyboard_focus_enter" for r in records(log))
                 and any(r.get("event") == "query_frame" and r["files"] == 15 for r in records(log)),
                 "launcher focused with daemon results", process=p)
        wait_bounds(log, bounds, p)
        return p, log

    def wait_bounds(log, bounds, p):
        wait_for(lambda: [r for r in records(log) if r.get("event") == "output_geometry"][-1:]
                 and [r for r in records(log) if r.get("event") == "output_geometry"][-1]["width"] == bounds[0]
                 and [r for r in records(log) if r.get("event") == "output_geometry"][-1]["height"] == bounds[1],
                 "entered output logical bounds", process=p)
        wait_for(lambda: any(r.get("event") == "surface_resized" and r["width"] == bounds[2]
                             and r["height"] == bounds[3] for r in records(log)), "surface fits selected output", process=p)

    def close(p, index):
        subprocess.run([str(binary)], check=True, timeout=5)
        assert p.wait(timeout=5) == 0
        wait_for(lambda: not owner(), "launcher released its bus name")
        path = out / f"probe-{index}.jsonl"
        wait_for(lambda: any(r.get("event") == "focus" for r in records(path)), "previous app regained focus")
        type_text("z")
        wait_for(lambda: {"text": "pz" if index == 0 else "sz"} in records(path), "typing returned to previous app")

    try:
        initial = json.loads(doctor("-j"))
        (out / "initial-outputs.json").write_text(json.dumps(initial, indent=2))
        devices = sorted(initial["outputs"], key=lambda entry: entry["id"])
        assert len(devices) == 2 and all(d["enabled"] for d in devices)
        names = [d["name"] for d in devices]
        both = [([0,0,1280,800],1.0), ([1280,0,853,533],1.5)]
        dense = [([0,0,1280,800],1.0), ([1280,0,640,400],2.0)]
        apply("initial", both, f"output.{names[0]}.position.0,0", f"output.{names[1]}.position.1280,0", f"output.{names[1]}.scale.1.5")
        hold = subprocess.Popen([str(helper), "--hold"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        processes.append(hold)
        wait_for(lambda: select.select([hold.stdout],[],[],0)[0], "persistent input seat")
        assert hold.stdout.readline().strip() == "ready"
        start_probe(0)
        second_probe = start_probe(1)
        cases = []
        for mode in MODES:
            configuration.write_bytes(original + b'\n[appearance]\nmode="dark"\ndark_theme="slate"\nwidth="wide"\nposition="centered"\n')
            for index, bounds, rectangle in [(0,(1280,800,880,501),(200,149.5,880,501)),
                                              (1,(853,533,821,501),(16,16,821,501))]:
                focus(index)
                p, log = launch(mode, str(index), bounds)
                keys(14,-14,20,-20)  # Backspace + t proves main input owns focus.
                wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 3 for r in records(log)), "typing after opening on selected monitor", process=p)
                capture(f"{mode}-monitor-{index}", rectangle, 1 if index == 0 else 1.5)
                if index == 1:
                    apply(f"dense-{mode}", dense, f"output.{names[1]}.scale.2")
                    wait_bounds(log, (640,400,608,339), p)
                    capture(f"{mode}-scale-2", (16,30.5,608,339), 2)
                    apply(f"restore-{mode}", both, f"output.{names[1]}.scale.1.5")
                    wait_bounds(log, bounds, p)
                    chord(15)
                    chord(15)
                    wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 5 for r in records(log)), "file target selected", process=p)
                    chord(37)
                    wait_for(lambda: any(r.get("event") == "actions_focused" for r in records(log)), "preview actions focused", process=p)
                    type_text("preview")
                    keys(28,-28)
                    wait_for(lambda: any(r.get("event") == "preview_loaded" for r in records(log)), "preview after live scale change", process=p)
                    capture(f"{mode}-preview", rectangle, 1.5)
                close(p,index)
            focus(1)
            p, log = launch(mode,"disconnect",(853,533,821,501))
            keys(29,42,51,-51,-42,-29)
            wait_for(lambda: any(r.get("event") == "appearance_ready" for r in records(log)), "appearance loaded before output removal", process=p)
            keys(108,-108,28,-28)  # Unsaved Light draft; the compositor will close its surface.
            saved = configuration.read_bytes()
            apply(f"disabled-{mode}",[([0,0,1280,800],1.0)],f"output.{names[1]}.disable")
            wait_for(lambda: p.poll() is not None, "removed output closed its launcher", process=p)
            assert p.returncode == 0 and not owner(), "removed output left an invisible launcher owning the toggle"
            assert configuration.read_bytes() == saved, "removing an output must not save the appearance draft"
            second_probe.terminate()
            second_probe.wait(timeout=5)
            focus(0)
            p,log = launch(mode,"remaining",(1280,800,880,501))
            close(p,0)
            apply(f"enabled-{mode}",both,f"output.{names[1]}.enable",f"output.{names[1]}.position.1280,0",f"output.{names[1]}.scale.1.5")
            second_probe = start_probe(1)
            focus(1)
            p,log = launch(mode,"reconnected",(853,533,821,501))
            close(p,1)
            cases.append({"mode":mode,"validation":"passed"})
        (out / "multimonitor.json").write_text(json.dumps({"topology":snapshots,"cases":cases,"validation":"passed"},indent=2)+"\n")
        print("OK: distinct mixed-scale outputs, focus, resizing, preview, disable/re-enable and reopening")
    finally:
        configuration.write_bytes(original)
        for p in reversed(processes):
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()


def main():
    if (os.environ.get("FERROSEEK_SMOKE_ISOLATED") != "1"
            or not os.environ.get("WAYLAND_DISPLAY", "").startswith("wayland-ferroseek-smoke-")):
        raise SystemExit("Run through FERROSEEK_SMOKE_INTERACTION=1 tests/launcher-smoke.sh")
    binary, out, helper = map(Path, sys.argv[1:4])
    if os.environ.get("FERROSEEK_SMOKE_PROFILE") == "us-multi":
        return multimonitor(binary, out, helper)
    trials = int(os.environ.get("FERROSEEK_SMOKE_TRIALS", "20"))
    assert 2 <= trials <= 100
    requested_mode = os.environ.get("FERROSEEK_SMOKE_MODE", "auto")
    mode_names = list(MODES) if requested_mode == "all" else [requested_mode]
    assert all(mode in MODES for mode in mode_names), "unknown launcher mode"
    functional_mode = mode_names[0]
    schedule = [mode for repetition in range(trials)
                for mode in mode_names[repetition % len(mode_names):] + mode_names[:repetition % len(mode_names)]]
    probe_log = out / "focus-probe.jsonl"
    processes = []
    samples = []
    activation_log = out / "activation-target.jsonl"
    profile = os.environ.get("FERROSEEK_SMOKE_PROFILE", "us-1x")
    layout_only = profile in ("us-layout-1x", "us-layout-1.5x", "us-short-1x")
    assert layout_only or profile in ["us-1x", "fr-1x", "us-1.5x"], "unknown launcher profile"
    layout = "fr" if profile == "fr-1x" else "us"
    requested_scale = 1.5 if profile.endswith("1.5x") else 1.0
    support = ast.literal_eval(subprocess.check_output([
        "gdbus", "call", "--session", "--dest", "org.kde.KWin", "--object-path", "/KWin",
        "--method", "org.kde.KWin.supportInformation"], text=True, timeout=5))[0]
    (out / "kwin-support.txt").write_text(support)
    if not re.search(r"(?m)^Compositing Type: OpenGL\s*$", support):
        print("INCONCLUSIVE: material checks require OpenGL compositing; see kwin-support.txt", file=sys.stderr)
        raise SystemExit(77)
    screen_count = re.findall(r"(?m)^Number of Screens: (\d+)\s*$", support)
    geometries = re.findall(r"(?m)^Geometry: (-?\d+),(-?\d+),(\d+)x(\d+)\s*$", support)
    scales = re.findall(r"(?m)^Scale: ([0-9.]+)\s*$", support)
    assert screen_count == ["1"] and len(geometries) == len(scales) == 1, "expected exactly one observed output"
    geometry = [int(value) for value in geometries[0]]
    observed_scale = float(scales[0])
    expected_geometry = ([0, 0, 640, 360] if profile == "us-short-1x" else
                         [0, 0, 640, 480] if profile == "us-layout-1x" else
                         [0, 0, 960, 640] if profile == "us-layout-1.5x" else [0, 0, 1280, 800])
    assert geometry == expected_geometry, "unexpected logical output geometry"
    assert observed_scale == requested_scale, "requested scale differs from actual compositor scale"
    character_keys = dict(zip("abcdefghijklmnopqrstuvwxyz", [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50,
        49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44]))
    if layout == "fr":
        character_keys.update(a=16, q=30, z=17, w=44, m=39)

    def keys(*codes):
        subprocess.run([str(helper), *map(str, codes)], check=True, timeout=5)

    def tap(code):
        keys(code, -code)

    def chord(modifier, code):
        keys(modifier, code, -code, -modifier)

    def type_text(value):
        for character in value:
            tap(character_keys[character])

    def text_report():
        type_text("report")

    def text_codes(value):
        return [code for character in value for code in (character_keys[character], -character_keys[character])]

    def control_codes(character):
        code = character_keys[character]
        return [29, code, -code, -29]

    def burst(*codes):
        subprocess.run([str(helper), "--burst", *map(str, codes)], check=True, timeout=5)

    def capture(name):
        with (out / (name + ".log")).open("w") as capture_log:
            subprocess.run(["spectacle", "-b", "-n", "-f", "-o", str(out / name)],
                           check=True, timeout=15, stdout=capture_log, stderr=capture_log,
                           env={**os.environ, "XDG_DATA_DIRS": "/usr/local/share:/usr/share",
                                "XDG_CONFIG_DIRS": "/etc/xdg"})
        assert (out / name).is_file(), "capture tool returned without an image; inspect its companion log"
        png = (out / name).read_bytes()
        assert png[:8] == b"\x89PNG\r\n\x1a\n"
        assert struct.unpack(">II", png[16:24]) == (int(geometry[2] * observed_scale), int(geometry[3] * observed_scale)), \
            "screenshot must capture the configured output at native resolution"

    def owner(name="io.github.ferroseek.Launcher"):
        result = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                                 "--object-path", "/org/freedesktop/DBus", "--method",
                                 "org.freedesktop.DBus.NameHasOwner", name],
                                check=True, capture_output=True, text=True, timeout=5)
        return "true" in result.stdout

    probe_process = subprocess.Popen([sys.executable, __file__, "--probe", str(probe_log)])
    processes.append(probe_process)
    try:
        # A physical input device remains connected after a click. Keep the
        # synthetic seat capabilities alive across short command-client calls.
        input_device = subprocess.Popen([str(helper), "--hold"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, text=True)
        processes.append(input_device)
        wait_for(lambda: select.select([input_device.stdout], [], [], 0)[0], "input device registered")
        assert input_device.stdout.readline().strip() == "ready"
        wait_for(lambda: records(probe_log), "focus probe mapped")
        type_text("z")  # Establish ordinary input before launcher assertions.
        wait_for(lambda: {"text": "z"} in records(probe_log), "initial probe input")
        tap(2)  # Unshifted physical Digit1 proves the compositor's active layout.
        symbol = "&" if profile == "fr-1x" else "1"
        observed = wait_for(lambda: next((r["text"] for r in records(probe_log)
                                        if isinstance(r.get("text"), str) and len(r["text"]) == 2), None),
                            "unshifted Digit1 reached the input probe")
        keyboard_preflight = {"expected": "z" + symbol, "observed": observed}
        (out / "keyboard-preflight.json").write_text(json.dumps(keyboard_preflight, indent=2) + "\n")
        assert observed == keyboard_preflight["expected"], "requested keyboard layout is not active"
        chord(29, character_keys["a"])  # Logical Ctrl+A in the active layout
        tap(14)
        wait_for(lambda: {"text": ""} in records(probe_log), "probe cleared")

        if layout_only:
            import tomllib
            from PyQt5.QtGui import QImage
            configuration = Path(os.environ["FERROSEEK_CONFIG"])
            original = configuration.read_bytes()
            cases = []
            selected_handler = out / "selected-handler.txt"
            handler = out / "layout-handler"
            handler.write_text("#!/usr/bin/python3\nfrom pathlib import Path\nimport sys\n"
                               + f"Path({str(selected_handler)!r}).write_text(sys.argv[1])\n")
            handler.chmod(0o755)
            applications = Path(os.environ["XDG_DATA_HOME"]) / "applications"
            for index in range(7):
                (applications / f"layout-handler-{index}.desktop").write_text(
                    f"[Desktop Entry]\nType=Application\nName=Layout Handler {index} with a deliberately long name for readability\n"
                    f"MimeType=text/plain;\nExec={handler} {index} %f\n")
            # Worked examples in logical pixels, independent of the production calculator.
            small = geometry[2] == 640
            height, upper, centered = (435, 29, 22.5) if small else (501, 123, 69.5)
            widths = [608, 608, 608] if small else [640, 720, 880]

            def rectangle(name, width, top):
                capture(name)
                image = QImage(str(out / name))
                def dark(x, y):
                    color = image.pixelColor(round(x * observed_scale), round(y * observed_scale))
                    return max(color.red(), color.green(), color.blue()) < 150
                # The maximized input probe is light; the launcher is dark. Ignore
                # text and rounded corners by requiring most of a scanline to agree.
                ys = [y for y in range(geometry[3])
                      if sum(dark(x, y) for x in range(0, geometry[2], 4)) > width * 0.75 / 4]
                assert ys, "no dark launcher rectangle in native capture"
                xs = [x for x in range(geometry[2])
                      if sum(dark(x, y) for y in range(round(top + 40), round(top + height - 40), 8))
                      > (height - 80) / 8 * 0.7]
                actual = [min(xs), min(ys), max(xs) + 1, max(ys) + 1]
                expected = [(geometry[2] - width) / 2, top, (geometry[2] + width) / 2, top + height]
                assert all(abs(a - e) <= 3 for a, e in zip(actual, expected)), (name, actual, expected)
                return actual

            for mode in mode_names:
                configuration.write_bytes(original + b'\n[appearance]\nmode="dark"\nmaterial="glass"\ndark_theme="slate"\n')
                if profile == "us-short-1x":
                    log = out / f"chooser-one-row-{mode}.jsonl"
                    with log.open("w") as stream:
                        process = subprocess.Popen([str(binary), "--private", *MODES[mode].arguments,
                                                    "--query", "report"], stdout=stream, stderr=stream,
                                                   env=launcher_environment(mode))
                    processes.append(process)
                    wait_for(lambda: any(r.get("event") == "query_frame" and r["files"] == 15 for r in records(log))
                             and any(r.get("event") == "keyboard_focus_enter" for r in records(log)),
                             "short-output search ready", process=process)
                    chord(29, 15)
                    chord(29, 15)
                    wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 3 for r in records(log)),
                             "short-output Files scope ready", process=process)
                    chord(29, character_keys["k"])
                    wait_for(lambda: any(r.get("event") == "actions_focused" for r in records(log)), "short-output actions focused")
                    type_text("with")
                    tap(28)
                    wait_for(lambda: any(r.get("event") == "applications_loaded" for r in records(log)),
                             "one-row chooser loaded", process=process)
                    burst(29, 102, -102, -29, 109, -109, 109, -109, 104, -104)
                    capture(f"chooser-one-row-{mode}.png")
                    selected_handler.unlink(missing_ok=True)
                    chord(29, 2)
                    selected = wait_for(lambda: selected_handler.read_text() if selected_handler.exists() else None,
                                        "paged application dispatched")
                    assert selected == "1", "Page Down twice then Page Up must reach the second app when one row fits"
                    assert process.wait(timeout=5) == 0
                    wait_for(lambda: not owner(), "one-row chooser closed")
                    cases.append({"mode": mode, "one_row_paging": "passed"})
                    continue
                for restart in (False, True):
                    log = out / f"layout-{mode}-{int(restart)}.jsonl"
                    with log.open("w") as stream:
                        process = subprocess.Popen([str(binary), "--private", *MODES[mode].arguments,
                                                    "--query", "report"], stdout=stream, stderr=stream,
                                                   env=launcher_environment(mode))
                    processes.append(process)
                    wait_for(lambda: any(r.get("event") == "query_frame" and r["files"] == 15 for r in records(log)),
                             "layout results ready", process=process)
                    wait_for(lambda: any(r.get("event") == "keyboard_focus_enter" for r in records(log)),
                             "layout keyboard focus", process=process)
                    bounds = wait_for(lambda: next((r for r in records(log) if r.get("event") == "output_geometry"), None),
                                      "logical bounds of the entered output", process=process)
                    assert [bounds["width"], bounds["height"]] == geometry[2:]
                    if restart:
                        rectangle(f"layout-restored-{mode}.png", widths[2], centered)
                        # Search focus must survive the settings round trip and a restart.
                        tap(14)
                        wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 2 for r in records(log)),
                                 "typing after layout restart", process=process)
                        chord(29, 15)
                        chord(29, 15)
                        wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 4 for r in records(log)),
                                 "chooser Files scope ready", process=process)
                        chord(29, character_keys["k"])
                        wait_for(lambda: any(r.get("event") == "actions_focused" for r in records(log)), "chooser actions focused")
                        type_text("with")
                        tap(28)
                        wait_for(lambda: any(r.get("event") == "applications_loaded" for r in records(log)),
                                 "seven long application names loaded", process=process)
                        burst(29, 107, -107, -29)  # Last application; numbers follow visible rows.
                        capture(f"layout-chooser-last-{mode}.png")
                        selected_handler.unlink(missing_ok=True)
                        chord(29, 2)  # First visible numbered choice, after scrolling to the last.
                        selected = wait_for(lambda: selected_handler.read_text() if selected_handler.exists() else None,
                                            "visible chooser application dispatched")
                        assert selected == ("2" if small else "1"), "chooser dispatched a row outside its constrained window"
                        # A successful Open with closes the launcher itself.
                        assert process.wait(timeout=5) == 0
                        wait_for(lambda: not owner(), "layout restart closed")
                        continue
                    # Files scope makes a previewable text file the selected target.
                    chord(29, 15)
                    chord(29, 15)
                    wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 3 for r in records(log)),
                             "Files scope ready", process=process)
                    starts = len([r for r in records(log) if r.get("event") == "query_start"])
                    burst(29, 42, 51, -51, -42, -29)
                    wait_for(lambda: any(r.get("event") == "appearance_ready" for r in records(log)),
                             "layout preferences loaded", process=process)
                    ready_at = next(r["elapsed_us"] for r in records(log) if r.get("event") == "appearance_ready")
                    wait_for(lambda: any(r.get("event") == "appearance_frame" and r["elapsed_us"] > ready_at for r in records(log)),
                             "appearance page rendered before pointer input", process=process)
                    # Pointer uses the same tab as Ctrl+Tab; coordinates are logical.
                    left = (geometry[2] - widths[1]) / 2
                    subprocess.run([str(helper), "--double-click", str(round(left + 235)), str(round(upper + 20))],
                                   check=True, timeout=5)

                    def choose(index):
                        before = len([r for r in records(log) if r.get("event") == "appearance_frame"])
                        burst(29, 102, -102, -29, *([108, -108] * index), 28, -28)
                        wait_for(lambda: len([r for r in records(log) if r.get("event") == "appearance_frame"]) > before,
                                 "layout choice redrawn", process=process)

                    for index, width in enumerate(widths):
                        choose(index)
                        rectangle(f"layout-width-{index}-{mode}.png", width, upper)
                    choose(4)  # Centered, live preview before saving.
                    rectangle(f"layout-centered-{mode}.png", widths[2], centered)
                    assert "width" not in tomllib.loads(configuration.read_text())["appearance"]
                    chord(29, character_keys["s"])
                    wait_for(lambda: any(r.get("event") == "appearance_saved" for r in records(log)),
                             "layout saved", process=process)
                    saved = tomllib.loads(configuration.read_text())
                    assert saved["appearance"]["width"] == "wide" and saved["appearance"]["position"] == "centered"
                    assert saved["appearance"]["dark_theme"] == "slate"
                    assert saved["roots"] == tomllib.loads(original.decode())["roots"]
                    choose(5)  # Reset previews only width and placement.
                    rectangle(f"layout-reset-{mode}.png", widths[1], upper)
                    assert tomllib.loads(configuration.read_text())["appearance"] == saved["appearance"]
                    tap(1)  # Back asks for discard.
                    chord(29, character_keys["d"])
                    wait_for(lambda: any(r.get("event") == "appearance_closed" for r in records(log)),
                             "layout reset discarded", process=process)
                    rectangle(f"layout-discard-{mode}.png", widths[2], centered)
                    assert len([r for r in records(log) if r.get("event") == "query_start"]) == starts
                    chord(29, character_keys["k"])
                    wait_for(lambda: any(r.get("event") == "actions_focused" for r in records(log)), "layout action focus")
                    capture(f"layout-actions-{mode}.png")
                    type_text("preview")
                    tap(28)
                    wait_for(lambda: any(r.get("event") == "preview_loaded" for r in records(log)),
                             "preview after layout settings", process=process)
                    rectangle(f"layout-preview-{mode}.png", 608 if small else 928, centered)
                    tap(14)
                    wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 4 for r in records(log)),
                             "search kept focus after settings and preview", process=process)
                    subprocess.run([str(binary)], check=True, timeout=5)
                    assert process.wait(timeout=5) == 0
                    wait_for(lambda: not owner(), "layout process closed")
                cases.append({"mode": mode, "widths": widths, "height": height,
                              "bounds": geometry[2:], "scale": observed_scale, "validation": "passed"})
            configuration.write_bytes(original)
            (out / "layout.json").write_text(json.dumps({"profile": profile, "cases": cases}, indent=2) + "\n")
            print("OK: layout presets, placement, reset/discard, persistence, preview and focus")
            return

        for trial, mode in enumerate(schedule):
            log = out / f"interaction-{trial + 1}.jsonl"
            with log.open("w") as stream:
                process = subprocess.Popen([str(binary), "--private", *MODES[mode].arguments],
                                           stdout=stream, stderr=stream, env=launcher_environment(mode))
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "first_frame" for r in records(log))
                         and any(r.get("event") == "keyboard_focus_enter" for r in records(log)),
                         "first redraw and compositor keyboard focus", process=process)
                text_report()  # No click or arbitrary delay; both readiness signals are observed.

                def frame(generation):
                    return next((r for r in records(log)
                                 if r.get("event") == "query_frame" and r["generation"] == generation), None)

                result = wait_for(lambda: frame(6), "six typed characters reached main input")
                assert result["files"] == 15, "the real daemon must supply the capped file results"
                first = next(r for r in records(log) if r.get("event") == "first_frame")
                ready = next(r for r in records(log)
                             if r.get("event") == "query_ready" and r["generation"] == result["generation"])
                assert 0 <= ready["query_us"] <= result["query_us"], "results must precede their redraw event"
                renderer = wait_for(lambda: observed_renderer(log), "renderer observed in a draw", process=process)
                if mode == "software":
                    assert renderer == "tiny_skia", "forced software must actually draw with tiny-skia"
                focus = next(r for r in records(log) if r.get("event") == "keyboard_focus_enter")
                memory = memory_snapshot(process.pid)
                samples.append(LauncherSample(
                    trial=trial + 1, mode=mode, renderer=renderer, generation=result["generation"],
                    first_frame_us=first["elapsed_us"], keyboard_focus_us=focus["elapsed_us"],
                    query_ready_us=ready["query_us"], ready_to_frame_us=result["query_us"] - ready["query_us"],
                    query_frame_us=result["query_us"], rss_snapshot_kib=memory["VmRSS"],
                    hwm_snapshot_kib=memory["VmHWM"]))
                with (out / "samples.jsonl").open("a") as snapshots:
                    snapshots.write(json.dumps(asdict(samples[-1])) + "\n")
                # Test focus after the asynchronous reply before any command can refocus a field.
                tap(14)
                edited = wait_for(lambda: frame(7), "main input kept focus after asynchronous results")
                assert edited["files"] == 15

                if trial == 0:
                    capture("interaction-results.png")
                    chord(29, 37)  # Ctrl+K, then type into the focused action filter.
                    wait_for(lambda: any(r.get("event") == "actions_focused"
                                         for r in records(log)), "action filter focused")
                    tap(19)
                    wait_for(lambda: any(r.get("event") == "action_input" for r in records(log)),
                             "action filter owns focus")
                    assert len([r for r in records(log) if r.get("event") == "query_start"]) == 7
                    capture("interaction-actions.png")
                    # Cursor blinks repaint only damaged regions; the panel must
                    # keep its raised background in every frame, not just full redraws.
                    from PyQt5.QtGui import QImage
                    for blink in range(4):
                        time.sleep(0.3)
                        capture(f"interaction-actions-blink-{blink}.png")
                        # Sample the same logical panel location in each native-resolution capture.
                        color = QImage(str(out / f"interaction-actions-blink-{blink}.png")).pixelColor(
                            int(900 * observed_scale), int(634 * observed_scale))
                        assert color.getRgb()[:3] == (0x22, 0x23, 0x29), \
                            f"action panel background {color.getRgb()[:3]} after a partial redraw"
                    tap(1)  # Escape clears the filter
                    wait_for(lambda: any(r.get("event") == "action_filter_cleared" for r in records(log)),
                             "Escape cleared the action filter")
                    tap(1)  # Escape closes actions
                    wait_for(lambda: any(r.get("event") == "actions_closed" for r in records(log)),
                             "Escape closed the action panel")
                    tap(14)  # Backspace must now edit the main field
                    wait_for(lambda: frame(8), "main input regained focus after closing actions")
                    capture("interaction-return.png")
                    # No synthetic UI messages: the public toggle closes the real instance.
                    subprocess.run([str(binary), "--private"], check=True, timeout=5)
                else:
                    tap(1)  # clear the query
                    wait_for(lambda: frame(8), "Escape cleared the query")
                    if trial == 1:
                        # Below the panel, on the maximized probe: it takes keyboard
                        # focus, which closes the launcher.
                        capture("interaction-click-outside.png")
                        subprocess.run([str(helper), "--double-click", "640", "760"], check=True, timeout=5)
                        wait_for(lambda: any(r.get("event") == "keyboard_focus_leave" for r in records(log)),
                                 "click outside took keyboard focus")
                    else:
                        tap(1)  # close

                assert process.wait(timeout=5) == 0
                wait_for(lambda: not owner(), "launcher name released")
                type_text("zz")
                wait_for(lambda: {"text": "zz"} in records(probe_log), "focus returned to previous application")
                chord(29, character_keys["a"])
                tap(14)
                wait_for(lambda: {"text": ""} in records(probe_log), "probe reset")

        def summary(values):
            values = sorted(values)
            return {"n": len(values), "p50": values[math.ceil(len(values) * .5) - 1],
                    "p95": values[math.ceil(len(values) * .95) - 1], "max": values[-1]} if values else None

        grouped = {}
        for sample in samples:
            grouped.setdefault(f"{sample.mode}/{sample.renderer}", []).append(asdict(sample))
        metrics = ("first_frame_us", "keyboard_focus_us", "query_ready_us", "ready_to_frame_us",
                   "query_frame_us", "rss_snapshot_kib", "hwm_snapshot_kib")
        summaries = {key: {"first_trial": group[0],
                           "repeats": {field: summary([s[field] for s in group[1:]]) for field in metrics}}
                     for key, group in grouped.items()}
        with binary.open("rb") as executable:
            launcher_sha256 = hashlib.file_digest(executable, "sha256").hexdigest()
        report = {"schema": 3, "validation": "incomplete", "input_burst_checks": True, "environment": {"platform": platform.platform(),
                  "cpu": subprocess.check_output(["lscpu"], text=True),
                  "kwin": subprocess.check_output(["kwin_wayland", "--version"], text=True).strip(),
                  "launcher_sha256": launcher_sha256,
                  "graphics_environment": {key: os.environ.get(key) for key in
                      ("WGPU_BACKEND", "WGPU_POWER_PREF", "ICED_PRESENT_MODE", "LIBGL_ALWAYS_SOFTWARE")},
                  "profile": profile, "keyboard_layout": layout,
                  "keyboard_preflight": keyboard_preflight,
                  "output": {"logical_geometry": geometry, "scale": observed_scale}},
                  "corpus_files": 10007, "requested_mode": requested_mode, "trials_per_mode": trials,
                  "modes": {mode: asdict(MODES[mode]) for mode in mode_names},
                  "samples": [asdict(sample) for sample in samples], "summaries": summaries,
                  "functional_mode": functional_mode,
                  "limits": (
                      "Diagnostic redraw callback and compositor focus notification, not completed drawing or presentation. "
                      "Warm daemon; synthetic corpus; keyboard layout verified by input. "
                      "Requested modes share one compositor; no cold-cache claim. "
                      "first_frame_us and keyboard_focus_us start at main entry; query_ready_us and query_frame_us "
                      "start at the final edit; ready_to_frame_us starts when results are applied. "
                      "RSS/HWM are /proc snapshots after results, before more input, including shared pages and excluding GPU memory. "
                      "Renderer events identify WGPU or tiny-skia, not the graphics adapter. Material pixels use separate processes.")}
        (out / "latency.json").write_text(json.dumps(report, indent=2) + "\n")

        # These processes are functional checks, separate from the measured
        # workload. Send each sequence without per-key server roundtrips or a
        # widget-focus wait between a shortcut/click and the following text.
        input_cases = []
        for mode in mode_names:
            log = out / f"input-burst-{mode}.jsonl"
            with log.open("w") as stream:
                process = subprocess.Popen([str(binary), "--private", *MODES[mode].arguments],
                                           stdout=stream, stderr=stream, env=launcher_environment(mode))
            processes.append(process)
            wait_for(lambda: any(r.get("event") == "keyboard_focus_enter" for r in records(log)),
                     "burst launcher owns compositor focus", process=process)
            burst(*text_codes("report"))
            wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 6 and r["files"] == 15
                                 for r in records(log)), "opening input burst reached search", process=process)
            burst(*control_codes("k"), *text_codes("pin"))
            wait_for(lambda: len([r for r in records(log) if r.get("event") == "action_input"]) == 3,
                     "immediate action filter input", process=process)
            assert len([r for r in records(log) if r.get("event") == "query_start"]) == 6
            burst(*control_codes("k"), 14, -14, *text_codes("t"),
                  *control_codes("k"), *text_codes("open"), 1, -1, 1, -1, 14, -14, *text_codes("t"))
            wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] == 10 and r["files"] == 15
                                 for r in records(log)), "burst returned to the original search", process=process)
            assert len([r for r in records(log) if r.get("event") == "action_input"]) == 7
            assert len([r for r in records(log) if r.get("event") == "action_filter_cleared"]) == 1
            assert len([r for r in records(log) if r.get("event") == "actions_closed"]) == 2
            burst("--right-click", 500, 295, *text_codes("launch"))
            wait_for(lambda: len([r for r in records(log) if r.get("event") == "action_input"]) == 13,
                     "right-click and immediate action text", process=process)
            assert len([r for r in records(log) if r.get("event") == "query_start"]) == 10
            capture(f"input-burst-actions-{mode}.png")
            current = records(log)
            field_ready = next(r for r in current if r.get("event") == "search_focused")
            focus = next(r for r in current if r.get("event") == "keyboard_focus_enter")
            input_cases.append({"mode": mode, "renderer": observed_renderer(log),
                                "keyboard_focus_us": focus["elapsed_us"],
                                "search_widget_focus_us": field_ready["elapsed_us"],
                                "query_edits": sum(r.get("event") == "query_start" for r in current),
                                "action_edits": sum(r.get("event") == "action_input" for r in current),
                                "action_filter_clears": sum(r.get("event") == "action_filter_cleared" for r in current),
                                "action_closes": sum(r.get("event") == "actions_closed" for r in current)})
            subprocess.run([str(binary), "--private"], check=True, timeout=5)
            assert process.wait(timeout=5) == 0
            wait_for(lambda: not owner(), "burst launcher name released")
        (out / "input-readiness.json").write_text(json.dumps({"schema": 1, "validation": "passed",
            "profile": profile, "cases": input_cases,
            "limits": "Input is injected only after compositor keyboard Enter. Widget focus is not presentation. No physical shortcut, IME or pre-focus capture claim."}, indent=2) + "\n")

        # Real keyboard navigation, saving and reopening with synthetic settings.
        import tomllib
        from PyQt5.QtGui import QImage
        configuration = Path(os.environ["FERROSEEK_CONFIG"])
        original_configuration = configuration.read_bytes()
        appearance_cases = []
        for mode in mode_names:
            for restart in (False, True):
                log = out / f"appearance-{mode}-{int(restart)}.jsonl"
                process = subprocess.Popen([str(binary), "--private", *MODES[mode].arguments, "--query", "report"],
                    stdout=log.open("w"), stderr=subprocess.STDOUT, env=launcher_environment(mode))
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "query_frame" and r["files"] == 15 for r in records(log)), "appearance search ready", process=process)
                wait_for(lambda: any(r.get("event") == "keyboard_focus_enter" for r in records(log)), "appearance keyboard focus", process=process)
                if restart:
                    capture(f"appearance-restored-{mode}.png")
                    color = QImage(str(out / f"appearance-restored-{mode}.png")).pixelColor(round(640*observed_scale), round(210*observed_scale))
                    assert max(abs(actual-expected) for actual,expected in zip((color.red(),color.green(),color.blue()),(239,243,248))) <= 2, "saved Slate Light theme did not draw on restart"
                burst(29,42,51,-51,-42,-29)  # Ctrl+Shift+physical Comma.
                wait_for(lambda: any(r.get("event") == "appearance_ready" for r in records(log)), "appearance loaded", process=process)
                def advance(count):
                    burst(*([108,-108] * count),28,-28)
                if not restart:
                    advance(1)  # Light
                    advance(2)  # Solid
                    advance(3)  # Slate light theme
                else:
                    advance(2)  # Dark
                    advance(2)  # Glass (after starting Solid)
                    advance(6)  # Forest dark theme
                burst(*control_codes("s"))
                wait_for(lambda: any(r.get("event") == "appearance_saved" for r in records(log)), "appearance saved", process=process)
                saved_at = next(r["elapsed_us"] for r in records(log) if r.get("event") == "appearance_saved")
                wait_for(lambda: any(r.get("event") == "appearance_frame" and r["elapsed_us"] > saved_at for r in records(log)), "appearance redrawn", process=process)
                name = f"appearance-{'dark' if restart else 'light'}-{mode}.png"
                capture(name)
                color = QImage(str(out / name)).pixelColor(round(640*observed_scale), round(210*observed_scale))
                assert (max(color.red(),color.green(),color.blue()) < 120 if restart else min(color.red(),color.green(),color.blue()) > 220), "appearance pixels disagree with selected mode"
                if restart and mode != "auto":
                    assert max(abs(actual-expected) for actual,expected in zip((color.red(),color.green(),color.blue()),(24,34,30))) <= 2, "opaque/software fallback lost the Forest palette"
                saved = tomllib.loads(configuration.read_text())["appearance"]
                assert saved["mode"] == ("dark" if restart else "light")
                assert saved["material"] == ("glass" if restart else "solid")
                assert saved["light_theme"] == "slate"
                if restart:
                    assert saved["dark_theme"] == "forest"
                    blur = any(r.get("event") == "material_blur_requested" for r in records(log))
                    assert blur == (mode != "opaque"), "live Glass toggle or --opaque override failed"
                burst(1,-1)
                wait_for(lambda: any(r.get("event") == "appearance_closed" for r in records(log)), "appearance returned to results", process=process)
                assert not any(r.get("event") == "query_start" and r["generation"] > 1 for r in records(log)), "settings input changed the search"
                subprocess.run([str(binary), "--private"], check=True, timeout=5)
                assert process.wait(timeout=5) == 0
                wait_for(lambda: not owner(), "appearance process closed")
            configuration.write_bytes(original_configuration)
            appearance_cases.append({"mode": mode, "persistence": "passed", "keyboard": "passed", "renderer": observed_renderer(log)})
        (out / "appearance.json").write_text(json.dumps({"validation": "passed", "profile": profile, "cases": appearance_cases}, indent=2) + "\n")

        # Launch a real single-instance target, then reactivate it from another app.
        target = Path(__file__).resolve().with_name("launcher-activation-probe.py")
        entry = Path(os.environ["XDG_DATA_HOME"]) / "applications/org.ferroseek.ActivationProbe.desktop"
        entry.write_text("[Desktop Entry]\nType=Application\nName=Activation Probe\n"
                         f'Exec="{sys.executable}" "{target}" %f\nMimeType=text/plain;\nActions=Numbered;\n'
                         '[Desktop Action Numbered]\nName=Numbered action\n'
                         f'Exec=/usr/bin/env FERROSEEK_TEST_ACTION=numbered "{sys.executable}" "{target}"\n')
        serving_pid = None
        for activation_number, mode in enumerate(["new", "existing", "pointer", "numbered", "action-numbered"], start=1):
            existing = mode != "new"
            if existing:
                # A new unrelated window must own input before requesting the target again.
                background = out / f"activation-background-{mode}.jsonl"
                background_process = subprocess.Popen([sys.executable, __file__, "--probe", str(background)])
                processes.append(background_process)
                wait_for(lambda: records(background), "second background window")
                type_text("z")
                wait_for(lambda: {"text": "z"} in records(background), "unrelated window owns input")
            log = out / f"activate-{mode}.jsonl"
            with log.open("w") as stream:
                process = subprocess.Popen(
                    [str(binary), "--private", "--query", "activation", *MODES[functional_mode].arguments],
                    stdout=stream, stderr=stream,
                    env=launcher_environment(functional_mode, FERROSEEK_ACTIVATION_LOG=str(activation_log),
                                             GSK_RENDERER="cairo", GDK_BACKEND="wayland", GTK_A11Y="none"),
                )
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "query_frame" for r in records(log))
                         and any(r.get("event") == "keyboard_focus_enter" for r in records(log)),
                         "target selected and compositor keyboard focus received", process=process)
                query_edits = len([r for r in records(log) if r.get("event") == "query_start"])
                if mode == "action-numbered":
                    chord(29, 37)
                    wait_for(lambda: any(r.get("event") == "actions_focused"
                                         for r in records(log)), "numbered action field focused")
                    type_text("numbered")
                    wait_for(lambda: len([r for r in records(log) if r.get("event") == "action_input"]) == 8,
                             "numbered action filter owns focus")
                    capture("activation-action-numbered-selection.png")
                    chord(29, 2)  # Physical Ctrl+Digit1, regardless of its printed symbol.
                elif mode == "numbered":
                    capture("activation-numbered-selection.png")
                    # Ordinary editing proves this instance's main widget owns focus.
                    tap(14)
                    wait_for(lambda: len([r for r in records(log) if r.get("event") == "query_start"])
                             == query_edits + 1, "numbered main field accepts Backspace")
                    type_text("n")  # Restore the preset query, activation.
                    wait_for(lambda: len([r for r in records(log) if r.get("event") == "query_start"])
                             == query_edits + 2, "numbered main field accepts ordinary typing")
                    query_edits += 2
                    chord(29, 2)
                elif mode == "pointer":
                    capture("activation-pointer-selection.png")
                    subprocess.run([str(helper), "--double-click", "400", "295"], check=True, timeout=5)
                else:
                    tap(28)  # Enter is the actual activation input.
                assert process.wait(timeout=5) == 0
                if mode in ["numbered", "action-numbered"]:
                    assert len([r for r in records(log) if r.get("event") == "query_start"]) == query_edits, \
                        "numbered shortcut must not edit the main query"
                if mode == "action-numbered":
                    assert len([r for r in records(log) if r.get("event") == "action_input"]) == 8, \
                        "numbered shortcut must not insert its symbol into the action filter"
                starts = wait_for(lambda: [r for r in records(activation_log) if r.get("event") == "started"],
                                  "target process started")
                if existing:
                    starts = wait_for(lambda: (s if len(s := [r for r in records(activation_log)
                                                              if r.get("event") == "started"]) == activation_number else None),
                                      "single-instance client started")
                assert starts[-1]["token_present"], "user-triggered launch must receive a fresh activation token"
                assert starts[-1]["action"] == ("numbered" if mode == "action-numbered" else None), \
                    "the selected numbered result/action must determine the command"
                activated = wait_for(lambda: (a if len(a := [r for r in records(activation_log)
                                                            if r.get("event") == "activated"]) >= activation_number
                                             else None), "application accepted activation")
                if existing:
                    assert activated[-1]["pid"] == serving_pid, "reactivate the existing instance"
                else:
                    serving_pid = activated[-1]["pid"]
                wait_for(lambda: any(r.get("event") == "focus" and r.get("active")
                                     and r.get("activation") == activation_number
                                     for r in records(activation_log)), "target window activated")
                type_text("z")
                expected = "z" * activation_number
                wait_for(lambda: any(r.get("event") == "input" and r.get("text") == expected
                                     for r in records(activation_log)), "chosen application owns keyboard focus")
                capture(f"activation-{mode}.png")

        # File actions use a fresh target process, then reactivate that instance.
        assert b"launcher-activation-probe.py" in Path(f"/proc/{serving_pid}/cmdline").read_bytes()
        os.kill(serving_pid, signal.SIGTERM)
        wait_for(lambda: not owner("org.ferroseek.ActivationProbe"), "application target closed")
        document = Path(os.environ["HOME"]) / "Corpus/Documents" / (
            "focus-document with a deliberately long descriptive filename for activation checks.txt")
        document.write_text("Synthetic activation document\n")
        mime = Path(os.environ["XDG_DATA_HOME"]) / "mime"
        mime.mkdir(exist_ok=True)
        (mime / "globs2").write_text("50:text/plain:*.txt\n")

        def indexed_document():
            result = subprocess.run(["gdbus", "call", "--session", "--dest", "io.github.ferroseek.FerroSeek",
                                     "--object-path", "/io/github/ferroseek/Search", "--method",
                                     "io.github.ferroseek.Search1.Search", "focus-document"],
                                    capture_output=True, text=True, timeout=5)
            return result.returncode == 0 and document.name in result.stdout

        wait_for(indexed_document, "new document indexed")
        for number, mode in enumerate(["file-new", "file-existing", "file-with", "file-pointer", "file-reveal"], 1):
            background = out / f"activation-background-{mode}.jsonl"
            background_process = subprocess.Popen([sys.executable, __file__, "--probe", str(background)])
            processes.append(background_process)
            wait_for(lambda: records(background), "file-action background window")
            type_text("z")
            wait_for(lambda: {"text": "z"} in records(background), "file-action background owns input")
            before_starts = len([r for r in records(activation_log) if r.get("event") == "started"])
            log = out / f"activate-{mode}.jsonl"
            with log.open("w") as stream:
                process = subprocess.Popen([str(binary), "--private", "--query", "focus-document",
                                            *MODES[functional_mode].arguments],
                                           stdout=stream, stderr=stream,
                                           env=launcher_environment(functional_mode))
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "query_frame" and r.get("files") == 1
                                     for r in records(log))
                         and any(r.get("event") == "keyboard_focus_enter" for r in records(log)),
                         "indexed document selected and compositor keyboard focus received", process=process)
                if mode == "file-with":
                    chord(29, 37)  # Ctrl+K; filter contextual actions to Open with.
                    wait_for(lambda: any(r.get("event") == "actions_focused"
                                         for r in records(log)), "Open with action field focused")
                    type_text("with")
                    wait_for(lambda: len([r for r in records(log) if r.get("event") == "action_input"]) == 4,
                             "Open with action filtered")
                    tap(28)
                    wait_for(lambda: any(r.get("event") == "applications_loaded" for r in records(log)),
                             "document handlers loaded")
                    type_text("activation")
                    wait_for(lambda: len([r for r in records(log) if r.get("event") == "application_input"]) == 10,
                             "chosen handler filtered")
                    capture("activation-file-with-selection.png")
                    tap(28)
                elif mode == "file-pointer":
                    capture("activation-file-pointer-selection.png")
                    subprocess.run([str(helper), "--double-click", "400", "295"], check=True, timeout=5)
                elif mode == "file-reveal":
                    wait_for(lambda: owner("org.freedesktop.FileManager1"), "synthetic file manager registered")
                    chord(29, 28)
                else:
                    tap(28)
                assert process.wait(timeout=5) == 0
                if mode == "file-reveal":
                    revealed = wait_for(lambda: [r for r in records(activation_log) if r.get("event") == "revealed"],
                                        "file manager accepted reveal")[-1]
                    assert revealed["uris"] == [document.as_uri()]
                    assert revealed["token_present"], "reveal must forward this operation's activation token"
                    assert revealed["pid"] == serving_pid
                else:
                    starts = wait_for(lambda: (rows if len(rows := [r for r in records(activation_log)
                                                                   if r.get("event") == "started"]) > before_starts
                                               else None), "document handler started")
                    assert starts[-1]["arguments"] == [str(document)], "preserve the selected document argument"
                    assert starts[-1]["token_present"], "file action must receive a fresh activation token"
                    opened = wait_for(lambda: (rows if len(rows := [r for r in records(activation_log)
                                                                   if r.get("event") == "opened"]) >= number
                                               else None), "document delivered to application")
                    assert opened[-1]["uris"] == [document.as_uri()]
                    if mode == "file-new":
                        serving_pid = opened[-1]["pid"]
                    assert opened[-1]["pid"] == serving_pid, "reuse the existing document handler"
                wait_for(lambda: any(r.get("event") == "focus" and r.get("active")
                                     and r.get("activation") == number and r.get("pid") == serving_pid
                                     for r in records(activation_log)), "document target activated")
                type_text("z")
                wait_for(lambda: any(r.get("event") == "input" and r.get("text") == "z" * number
                                     and r.get("pid") == serving_pid for r in records(activation_log)),
                         "document target owns keyboard focus")
                capture(f"activation-{mode}.png")

        # Fine stripes distinguish compositor blur from simple transparency.
        # The launcher samples are inside a blank results area, away from text.
        from PyQt5.QtGui import QImage
        backdrop_ready = out / "material-backdrop-ready"
        backdrop = subprocess.Popen([sys.executable, __file__, "--material-backdrop", str(backdrop_ready)])
        processes.append(backdrop)
        wait_for(backdrop_ready.exists, "material backdrop owns focus")
        capture("material-backdrop.png")

        def pixels(name, left, top, width, height):
            image = QImage(str(out / name))
            return [image.pixelColor(x, y).red()
                    for y in range(round(top * observed_scale), round((top + height) * observed_scale))
                    for x in range(round(left * observed_scale), round((left + width) * observed_scale))]

        from statistics import mean, pstdev
        stripe_variance = pstdev(pixels("material-backdrop.png", 400, 580, 160, 20))
        assert stripe_variance > 60, "backdrop must contain visible high-frequency detail"
        material_samples = {}
        for mode in ["auto", "opaque", "software"]:
            log = out / f"material-{mode}.jsonl"
            with log.open("w") as stream:
                launcher = subprocess.Popen([str(binary), "--query", "material-fixture-no-match",
                                             *MODES[mode].arguments], stdout=stream, stderr=stream,
                                            env=launcher_environment(mode))
            processes.append(launcher)
            wait_for(lambda: any(r.get("event") == "query_frame" for r in records(log)),
                     "material results frame", process=launcher)
            if mode == "auto":
                wait_for(lambda: any(r.get("event") == "material_blur_requested" for r in records(log)), "compositor blur available")
            capture(f"material-{mode}.png")
            values = pixels(f"material-{mode}.png", 400, 580, 160, 20)
            renderer = wait_for(lambda: observed_renderer(log), "material renderer observed", process=launcher)
            assert renderer == ("tiny_skia" if mode == "software" else "wgpu"), "material control renderer differs"
            material_samples[mode] = {"mean": mean(values), "stddev": pstdev(values), "renderer": renderer}
            if mode in ["opaque", "software"]:
                assert set(values) == {0x17}, "opaque mode must not expose the backdrop"
                if mode == "opaque":
                    assert not any(r.get("event") == "material_blur_requested" for r in records(log))
            else:
                assert mean(values) > 0x17 + 5, "glass must show the blurred backdrop"
                assert pstdev(values) < 3, "fine backdrop detail must be blurred, not just alpha blended"
                assert pixels(f"material-{mode}.png", 280, 200, 2, 2) == \
                    pixels("material-backdrop.png", 280, 200, 2, 2), "rounded corners must not blur outside the panel"

                before = len([r for r in records(log) if r.get("event") == "query_start"])
                chord(29, character_keys["a"])  # Ctrl+A in the active layout.
                text_report()
                wait_for(lambda: any(r.get("event") == "query_frame" and r["generation"] >= before + 6
                                     for r in records(log)), "file results for material preview")
                tap(108)  # First file, after the synthetic application.
                chord(29, 37)
                wait_for(lambda: any(r.get("event") == "actions_focused" for r in records(log)), "preview action filter")
                type_text("preview")
                wait_for(lambda: len([r for r in records(log) if r.get("event") == "action_input"]) == 7,
                         "preview filter received input")
                resizes = len([r for r in records(log) if r.get("event") == "surface_resized"])
                tap(28)
                wait_for(lambda: any(r.get("event") == "preview_loaded" for r in records(log)), "preview loaded")
                wait_for(lambda: any(r["width"] == 980 for r in
                                     [r for r in records(log) if r.get("event") == "surface_resized"][resizes:]),
                         "expanded preview surface observed", process=launcher)
                capture("material-preview.png")
                expanded = pixels("material-preview.png", 1040, 580, 60, 20)
                assert 0x17 + 5 < mean(expanded) < 60 and pstdev(expanded) < 3, \
                    "blur must extend into the expanded preview surface"
                resizes = len([r for r in records(log) if r.get("event") == "surface_resized"])
                tap(1)
                wait_for(lambda: any(r.get("event") == "preview_closed" for r in records(log)), "preview closed")
                wait_for(lambda: any(r["width"] == 720 for r in
                                     [r for r in records(log) if r.get("event") == "surface_resized"][resizes:]),
                         "restored result surface observed", process=launcher)
                capture("material-preview-closed.png")
                assert pixels("material-preview-closed.png", 1040, 580, 60, 20) == \
                    pixels("material-backdrop.png", 1040, 580, 60, 20), "closing preview must restore the smaller surface"
            subprocess.run([str(binary)], check=True, timeout=5)
            launcher.wait(timeout=5)
            assert launcher.returncode == 0
        (out / "material.json").write_text(json.dumps({"profile": profile, "scale": observed_scale,
            "backdrop_stddev": stripe_variance, "samples": material_samples,
            "limits": "Software-rendered isolated KWin; protocol fallback and capability withdrawal use an independent wire peer."}, indent=2) + "\n")
        expected_groups = {f"{mode}/{'tiny_skia' if mode == 'software' else 'wgpu'}" for mode in mode_names}
        assert set(summaries) == expected_groups, "controlled graphics run must retain each requested renderer group"
        report["validation"] = "passed"
        (out / "latency.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(summaries))
        print(f"OK: {len(samples)} opens, typing, action-filter focus, Escape/toggle/click outside, background focus restoration")
    finally:
        if sys.exc_info()[0] is not None:
            try:
                capture("interaction-failure.png")
            except (OSError, subprocess.SubprocessError):
                pass
        for pid in {r["pid"] for r in records(activation_log) if r.get("event") == "started"}:
            try:
                command = Path(f"/proc/{pid}/cmdline").read_bytes()
                if b"launcher-activation-probe.py" in command:
                    os.kill(pid, signal.SIGTERM)
            except (FileNotFoundError, ProcessLookupError):
                pass
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    if sys.argv[1:2] == ["--probe"]:
        probe(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None)
    elif sys.argv[1:2] == ["--material-backdrop"]:
        material_backdrop(sys.argv[2])
    else:
        main()
