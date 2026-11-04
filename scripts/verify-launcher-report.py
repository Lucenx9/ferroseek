#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check a completed launcher report against its original process logs."""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path


def load_records(path):
    records = []
    for line in path.read_text().splitlines():
        try:
            records.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--binary", type=Path)
    args = parser.parse_args()
    report = json.loads((args.directory / "latency.json").read_text())
    assert report["schema"] == 3, "expected schema 3"
    assert report.get("validation") == "passed", "report is incomplete; functional/material checks did not pass"
    samples = report["samples"]
    snapshots = [json.loads(line) for line in (args.directory / "samples.jsonl").read_text().splitlines()]
    assert snapshots == samples, "report samples differ from the per-process snapshots"
    modes = set(report["modes"])
    assert modes == ({"auto", "opaque", "software"} if report["requested_mode"] == "all"
                     else {report["requested_mode"]})
    assert Counter(sample["mode"] for sample in samples) == dict.fromkeys(modes, report["trials_per_mode"])
    assert [sample["trial"] for sample in samples] == list(range(1, len(samples) + 1))
    if args.binary:
        with args.binary.open("rb") as executable:
            digest = hashlib.file_digest(executable, "sha256").hexdigest()
        assert digest == report["environment"]["launcher_sha256"], "report belongs to another executable"

    groups = defaultdict(list)
    for sample in samples:
        records = load_records(args.directory / f"interaction-{sample['trial']}.jsonl")

        def event(name, generation=None):
            return next(record for record in records if record.get("event") == name
                        and (generation is None or record["generation"] == generation))

        generation = sample["generation"]
        ready = event("query_ready", generation)
        frame = event("query_frame", generation)
        assert frame["files"] == 15, "sample must contain real daemon file results"
        assert sample["first_frame_us"] == event("first_frame")["elapsed_us"]
        assert sample["keyboard_focus_us"] == event("keyboard_focus_enter")["elapsed_us"]
        assert sample["query_ready_us"] == ready["query_us"]
        assert sample["query_frame_us"] == frame["query_us"]
        assert sample["ready_to_frame_us"] == frame["query_us"] - ready["query_us"] >= 0
        backends = {record["event"] for record in records
                    if record.get("event") in {"renderer_wgpu", "renderer_tiny_skia"}}
        assert backends == {f"renderer_{sample['renderer']}"}, "sample backend differs from drawing logs"
        assert sample["renderer"] == ("tiny_skia" if sample["mode"] == "software" else "wgpu")
        assert 0 < sample["rss_snapshot_kib"] <= sample["hwm_snapshot_kib"]
        if sample["mode"] == "opaque":
            assert not any(record.get("event") == "material_blur_requested" for record in records)
        groups[f"{sample['mode']}/{sample['renderer']}"].append(sample)

    metrics = ("first_frame_us", "keyboard_focus_us", "query_ready_us", "ready_to_frame_us",
               "query_frame_us", "rss_snapshot_kib", "hwm_snapshot_kib")
    assert set(report["summaries"]) == set(groups)
    for key, group in groups.items():
        summary = report["summaries"][key]
        assert summary["first_trial"] == group[0]
        assert set(summary["repeats"]) == set(metrics)
        for field in metrics:
            values = sorted(sample[field] for sample in group[1:])
            n = len(values)
            expected = {"n": n, "p50": values[(n * 50 + 99) // 100 - 1],
                        "p95": values[(n * 95 + 99) // 100 - 1], "max": values[-1]} if n else None
            assert summary["repeats"][field] == expected, f"incorrect {key}/{field} aggregate"
    material = json.loads((args.directory / "material.json").read_text())
    assert material["profile"] == report["environment"]["profile"]
    for mode in ("auto", "opaque", "software"):
        expected = "tiny_skia" if mode == "software" else "wgpu"
        assert material["samples"][mode]["renderer"] == expected
        records = load_records(args.directory / f"material-{mode}.jsonl")
        backends = {record["event"] for record in records
                    if record.get("event") in {"renderer_wgpu", "renderer_tiny_skia"}}
        assert backends == {f"renderer_{expected}"}
        if mode == "auto":
            widths = [record["width"] for record in records if record.get("event") == "surface_resized"]
            expanded = widths.index(980)
            assert 720 in widths[expanded + 1:], "preview surface did not restore its logical width"
    readiness_path = args.directory / "input-readiness.json"
    if readiness_path.exists():
        readiness = json.loads(readiness_path.read_text())
        assert readiness["schema"] == 1 and readiness["validation"] == "passed"
        assert readiness["profile"] == report["environment"]["profile"]
        assert {case["mode"] for case in readiness["cases"]} == modes
        for case in readiness["cases"]:
            records = load_records(args.directory / f"input-burst-{case['mode']}.jsonl")
            for field, event in (("query_edits", "query_start"), ("action_edits", "action_input"),
                                 ("action_filter_clears", "action_filter_cleared"), ("action_closes", "actions_closed")):
                assert case[field] == sum(record.get("event") == event for record in records)
            for field, event in (("keyboard_focus_us", "keyboard_focus_enter"), ("search_widget_focus_us", "search_focused")):
                assert case[field] == next(record["elapsed_us"] for record in records if record.get("event") == event)
            assert any(record.get("event") == f"renderer_{case['renderer']}" for record in records)
    print(f"OK: {len(samples)} samples, {len(groups)} renderer groups, raw timings and aggregates verified")


if __name__ == "__main__":
    main()
