#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Real keyboard/focus checks and diagnostic latency samples in virtual KWin."""
import ast
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


def probe(path):
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
    field.show()
    field.setFocus()
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


def wait_for(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.01)
    raise AssertionError("Timed out: " + description)


def main():
    if (os.environ.get("FERROSEEK_SMOKE_ISOLATED") != "1"
            or not os.environ.get("WAYLAND_DISPLAY", "").startswith("wayland-ferroseek-smoke-")):
        raise SystemExit("Run through FERROSEEK_SMOKE_INTERACTION=1 tests/launcher-smoke.sh")
    binary, out, helper = map(Path, sys.argv[1:4])
    trials = int(os.environ.get("FERROSEEK_SMOKE_TRIALS", "20"))
    assert 2 <= trials <= 100
    probe_log = out / "focus-probe.jsonl"
    probe_process = subprocess.Popen([sys.executable, __file__, "--probe", str(probe_log)])
    processes = [probe_process]
    samples = []
    activation_log = out / "activation-target.jsonl"
    profile = os.environ.get("FERROSEEK_SMOKE_PROFILE", "us-1x")
    assert profile in ["us-1x", "fr-1x", "us-1.5x"], "unknown launcher profile"
    layout = "fr" if profile == "fr-1x" else "us"
    requested_scale = 1.5 if profile == "us-1.5x" else 1.0
    support = ast.literal_eval(subprocess.check_output([
        "gdbus", "call", "--session", "--dest", "org.kde.KWin", "--object-path", "/KWin",
        "--method", "org.kde.KWin.supportInformation"], text=True, timeout=5))[0]
    (out / "kwin-support.txt").write_text(support)
    screen_count = re.findall(r"(?m)^Number of Screens: (\d+)\s*$", support)
    geometries = re.findall(r"(?m)^Geometry: (-?\d+),(-?\d+),(\d+)x(\d+)\s*$", support)
    scales = re.findall(r"(?m)^Scale: ([0-9.]+)\s*$", support)
    assert screen_count == ["1"] and len(geometries) == len(scales) == 1, "expected exactly one observed output"
    geometry = [int(value) for value in geometries[0]]
    observed_scale = float(scales[0])
    assert geometry == [0, 0, 1280, 800], "unexpected logical output geometry"
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

    def capture(name):
        subprocess.run(["spectacle", "-b", "-n", "-f", "-o", str(out / name)],
                       check=True, timeout=15, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        png = (out / name).read_bytes()
        assert png[:8] == b"\x89PNG\r\n\x1a\n"
        assert struct.unpack(">II", png[16:24]) == (int(1280 * observed_scale), int(800 * observed_scale)), \
            "screenshot must capture the configured output at native resolution"

    def owner(name="io.github.ferroseek.Launcher"):
        result = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                                 "--object-path", "/org/freedesktop/DBus", "--method",
                                 "org.freedesktop.DBus.NameHasOwner", name],
                                check=True, capture_output=True, text=True, timeout=5)
        return "true" in result.stdout

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
        symbol = "&" if os.environ.get("FERROSEEK_SMOKE_PROFILE") == "fr-1x" else "1"
        wait_for(lambda: {"text": "z" + symbol} in records(probe_log), "requested keyboard layout is active")
        chord(29, character_keys["a"])  # Logical Ctrl+A in the active layout
        tap(14)
        wait_for(lambda: {"text": ""} in records(probe_log), "probe cleared")

        for trial in range(trials):
            log = out / f"interaction-{trial + 1}.jsonl"
            with log.open("w") as stream:
                process = subprocess.Popen([str(binary), "--private"], stdout=stream, stderr=stream,
                                           env={**os.environ, "FERROSEEK_LAUNCHER_TIMINGS": "1"})
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "first_frame" for r in records(log)), "first redraw")
                text_report()  # no click or arbitrary focus delay after the first redraw

                def frame(generation):
                    return next((r for r in records(log)
                                 if r.get("event") == "query_frame" and r["generation"] == generation), None)

                result = wait_for(lambda: frame(6), "six typed characters reached main input")
                assert result["files"] == 15, "the real daemon must supply the capped file results"
                first = next(r for r in records(log) if r.get("event") == "first_frame")
                samples.append({"trial": trial + 1, "first_frame_us": first["elapsed_us"],
                                "query_frame_us": result["query_us"]})
                # Test focus after the asynchronous reply before any command can refocus a field.
                tap(14)
                edited = wait_for(lambda: frame(7), "main input kept focus after asynchronous results")
                assert edited["files"] == 15

                if trial == 0:
                    capture("interaction-results.png")
                    chord(29, 37)  # Ctrl+K, then type into the action filter
                    tap(19)
                    wait_for(lambda: any(r.get("event") == "action_input" for r in records(log)),
                             "action filter owns focus")
                    assert len([r for r in records(log) if r.get("event") == "query_start"]) == 7
                    capture("interaction-actions.png")
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
                    tap(1)  # close

                assert process.wait(timeout=5) == 0
                wait_for(lambda: not owner(), "launcher name released")
                type_text("z")
                wait_for(lambda: {"text": "z"} in records(probe_log), "focus returned to previous application")
                chord(29, 30)
                tap(14)
                wait_for(lambda: {"text": ""} in records(probe_log), "probe reset")

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
                    [str(binary), "--private", "--query", "activation"], stdout=stream, stderr=stream,
                    env={**os.environ, "FERROSEEK_LAUNCHER_TIMINGS": "1",
                         "FERROSEEK_ACTIVATION_LOG": str(activation_log), "GSK_RENDERER": "cairo",
                         "GDK_BACKEND": "wayland", "GTK_A11Y": "none"},
                )
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "query_frame" for r in records(log))
                         and any(r.get("event") == "[DEBUG-desktop-focus]keyboard_enter" for r in records(log)),
                         "target selected and compositor keyboard focus received")
                query_edits = len([r for r in records(log) if r.get("event") == "query_start"])
                if mode == "action-numbered":
                    chord(29, 37)
                    type_text("numbered")
                    wait_for(lambda: len([r for r in records(log) if r.get("event") == "action_input"]) == 8,
                             "numbered action filter owns focus")
                    capture("activation-action-numbered-selection.png")
                    chord(29, 2)  # Physical Ctrl+Digit1, regardless of its printed symbol.
                elif mode == "numbered":
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
                process = subprocess.Popen([str(binary), "--private", "--query", "focus-document"],
                                           stdout=stream, stderr=stream,
                                           env={**os.environ, "FERROSEEK_LAUNCHER_TIMINGS": "1"})
                processes.append(process)
                wait_for(lambda: any(r.get("event") == "query_frame" and r.get("files") == 1
                                     for r in records(log))
                         and any(r.get("event") == "[DEBUG-desktop-focus]keyboard_enter" for r in records(log)),
                         "indexed document selected and compositor keyboard focus received")
                if mode == "file-with":
                    chord(29, 37)  # Ctrl+K; filter contextual actions to Open with.
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

        def summary(values):
            values = sorted(values)
            return {"n": len(values), "p50_us": values[math.ceil(len(values) * .5) - 1],
                    "p95_us": values[math.ceil(len(values) * .95) - 1], "max_us": values[-1]}

        # First process reported separately; no cache-dropping, so neither set is a cold-cache result.
        report = {"schema": 1, "environment": {"platform": platform.platform(),
                  "cpu": subprocess.check_output(["lscpu"], text=True),
                  "kwin": subprocess.check_output(["kwin_wayland", "--version"], text=True).strip(),
                  "profile": profile, "keyboard_layout": layout,
                  "output": {"logical_geometry": geometry, "scale": observed_scale}},
                  "corpus_files": 10007, "first_trial": samples[0], "samples": samples,
                  "repeat_first_frame": summary([s["first_frame_us"] for s in samples[1:]]),
                  "repeat_query_frame": summary([s["query_frame_us"] for s in samples[1:]]),
                  "limits": "Diagnostic redraw callback, not compositor presentation. Software-rendered virtual KWin; warm daemon; synthetic corpus; configured keyboard layout verified by ordinary input. Input dispatch and physical display latency are excluded."}
        (out / "latency.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({k: report[k] for k in ("first_trial", "repeat_first_frame", "repeat_query_frame")}))
        print(f"OK: {trials} opens, typing, action-filter focus, Escape/toggle, background focus restoration")
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
        probe(sys.argv[2])
    else:
        main()
