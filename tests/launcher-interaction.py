#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Real keyboard/focus checks and diagnostic latency samples in virtual KWin."""
import json
import math
import os
from pathlib import Path
import platform
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

    def keys(*codes):
        subprocess.run([str(helper), *map(str, codes)], check=True, timeout=5)

    def tap(code):
        keys(code, -code)

    def chord(modifier, code):
        keys(modifier, code, -code, -modifier)

    def text_report():
        for code in [19, 18, 25, 24, 19, 20]:  # report, US keymap
            tap(code)

    def capture(name):
        subprocess.run(["spectacle", "-b", "-n", "-f", "-o", str(out / name)],
                       check=True, timeout=15, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def owner():
        result = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                                 "--object-path", "/org/freedesktop/DBus", "--method",
                                 "org.freedesktop.DBus.NameHasOwner", "io.github.ferroseek.Launcher"],
                                check=True, capture_output=True, text=True, timeout=5)
        return "true" in result.stdout

    try:
        wait_for(lambda: records(probe_log), "focus probe mapped")
        tap(44)  # z: establish that input reaches the background test application
        wait_for(lambda: {"text": "z"} in records(probe_log), "initial probe input")
        chord(29, 30)  # Ctrl+A
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
                    tap(1)  # Escape closes actions
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
                tap(44)
                wait_for(lambda: {"text": "z"} in records(probe_log), "focus returned to previous application")
                chord(29, 30)
                tap(14)
                wait_for(lambda: {"text": ""} in records(probe_log), "probe reset")

        def summary(values):
            values = sorted(values)
            return {"n": len(values), "p50_us": values[math.ceil(len(values) * .5) - 1],
                    "p95_us": values[math.ceil(len(values) * .95) - 1], "max_us": values[-1]}

        # First process reported separately; no cache-dropping, so neither set is a cold-cache result.
        report = {"schema": 1, "environment": {"platform": platform.platform(),
                  "cpu": subprocess.check_output(["lscpu"], text=True),
                  "kwin": subprocess.check_output(["kwin_wayland", "--version"], text=True).strip()},
                  "corpus_files": 10007, "first_trial": samples[0], "samples": samples,
                  "repeat_first_frame": summary([s["first_frame_us"] for s in samples[1:]]),
                  "repeat_query_frame": summary([s["query_frame_us"] for s in samples[1:]]),
                  "limits": "Diagnostic redraw callback, not compositor presentation. Software-rendered virtual KWin; warm daemon; synthetic corpus; US keyboard. Input dispatch and physical display latency are excluded."}
        (out / "latency.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({k: report[k] for k in ("first_trial", "repeat_first_frame", "repeat_query_frame")}))
        print(f"OK: {trials} opens, typing, action-filter focus, Escape/toggle, background focus restoration")
    finally:
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
