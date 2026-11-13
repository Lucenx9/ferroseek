#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Extract startup milestones from a verified launcher interaction artifact."""
import csv
import json
from pathlib import Path
import sys

EVENTS = ("backend_ready", "applications_ready", "ui_starting", "first_frame", "keyboard_focus_enter")


def startup_times(path):
    times = {}
    for line in path.read_text().splitlines():
        if not line.startswith('{"event":'):
            continue
        record = json.loads(line)
        if record["event"] in EVENTS:
            times.setdefault(record["event"], record["elapsed_us"])
    assert set(times) == set(EVENTS), f"missing startup milestones: {path}"
    assert 0 <= times["backend_ready"] <= times["applications_ready"] <= times["ui_starting"]
    assert times["ui_starting"] <= min(times["first_frame"], times["keyboard_focus_enter"])
    return times


def main():
    directory = Path(sys.argv[1])
    report = json.loads((directory / "latency.json").read_text())
    assert report["schema"] == 3 and report["validation"] == "passed"
    writer = csv.writer(sys.stdout, lineterminator="\n")
    writer.writerow(["trial", "mode", "renderer", "first_in_mode", *EVENTS])
    seen = set()
    for sample in report["samples"]:
        times = startup_times(directory / f"interaction-{sample['trial']}.jsonl")
        assert times["first_frame"] == sample["first_frame_us"]
        assert times["keyboard_focus_enter"] == sample["keyboard_focus_us"]
        mode = sample["mode"]
        writer.writerow([sample["trial"], mode, sample["renderer"], int(mode not in seen),
                         *(times[event] for event in EVENTS)])
        seen.add(mode)


if __name__ == "__main__":
    main()
