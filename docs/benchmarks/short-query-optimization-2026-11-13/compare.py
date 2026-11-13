#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Compare two probe executables on fresh synthetic indexes; never use user data."""

import csv
import json
from pathlib import Path
import statistics
import subprocess
import sys


def run_probe(binary, work):
    work.mkdir()
    with (work / "samples.csv").open("w") as output:
        subprocess.run([str(binary), str(work)], stdout=output, check=True, timeout=120)
    with (work / "samples.csv").open() as source:
        rows = list(csv.reader(source))
    assert len(rows) == 330, f"incomplete probe: {work}"
    return rows


def record(rows, oracle, groups, run):
    variant, repetition = run
    for files, _, _, case, micros, count, fingerprint in rows:
        key = (int(files), case)
        result = (count, fingerprint)
        assert oracle.setdefault(key, result) == result, f"result mismatch: {variant} {key}"
        groups.setdefault((variant, *key, repetition), []).append(float(micros) / 1000)


def summarize(groups):
    medians = {}
    for (variant, files, case, _), values in groups.items():
        medians.setdefault((variant, files, case), []).append(statistics.median(values))
    return [
        {"variant": variant, "files": files, "case": case,
         "median_ms": statistics.median(values), "min_ms": min(values), "max_ms": max(values)}
        for (variant, files, case), values in sorted(medians.items())
    ]


def check_budget(summary):
    medians = {(row["variant"], row["files"], row["case"]): row["median_ms"] for row in summary}
    # A relative budget avoids tying this controlled diagnostic to one CPU.
    # Require a substantial gain on the two motivating 100k no-hit cases.
    for case in ("short-path-miss", "short-tokens-miss"):
        before = medians[("before", 100000, case)]
        after = medians[("after", 100000, case)]
        assert after < before * 0.8, f"performance budget failed: {case}: {before} -> {after} ms"


def main():
    before, after, work = map(lambda value: Path(value).resolve(), sys.argv[1:])
    work.mkdir()  # Refuse reuse, including partial runs.
    oracle, groups = {}, {}
    for repetition in range(5):
        order = [("before", before), ("after", after)]
        if repetition % 2:
            order.reverse()
        for variant, binary in order:
            rows = run_probe(binary, work / f"{repetition}-{variant}")
            record(rows, oracle, groups, (variant, repetition))
            print(f"completed {repetition} {variant}", flush=True)
    summary = summarize(groups)
    (work / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    check_budget(summary)
    print("PASS: 3300 responses agree; both relative performance budgets passed")


if __name__ == "__main__":
    main()
