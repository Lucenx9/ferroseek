### 100000 files (seed 42)

Corpus: 100000 files in 5000 directories (+3000 files in excluded `node_modules`/`.git` trees), generated in 1.8s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 2.29 s (43610 files/s) |
| ↳ CPU time (user+sys) | 1.74 s |
| ↳ peak RSS (VmHWM) | 17.4 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 69.8 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| initial index, max_cpu_percent=100: wall | 1.11 s (90126 files/s) |
| ↳ CPU time (user+sys) | 1.13 s |
| ↳ peak RSS (VmHWM) | 23.6 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 52.4 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| index size on disk: database / WAL | 25.2 MiB (251 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 9.0 MiB (2.7 MiB anon, 6.3 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.47 / p95 3.03 / p99 3.67 / max 4.71 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.67 / p95 3.04 / p99 3.68 / max 4.75 ms |
| RSS after query burst | 13.1 MiB |
| relevance (eligible 187 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.442 (1.324–1.523) | 1.579 | 0.99 | 15/15 |
| exact-name / D-Bus | 1.199 (1.190–1.414) | 1.598 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.20 / p95 1.41 / p99 1.60 / max 1.60 ms | — | — | — |
| path-only / library | 0.650 (0.648–0.658) | 0.674 | 1.00 | 15/15 |
| path-only / D-Bus | 0.765 (0.759–0.773) | 0.800 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.76 / p95 0.78 / p99 0.80 / max 0.80 ms | — | — | — |
| path+name / library | 0.650 (0.645–0.657) | 0.710 | 0.99 | 15/15 |
| path+name / D-Bus | 0.767 (0.740–0.802) | 0.870 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.77 / p95 0.81 / p99 0.87 / max 0.87 ms | — | — | — |
| within+name / library | 0.054 (0.052–0.058) | 0.118 | 1.00 | 15/15 |
| path-miss / library | 0.272 (0.267–0.307) | 0.313 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.366 (0.336–0.426) | 0.437 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.37 / p95 0.43 / p99 0.44 / max 0.44 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 1.97 | 15/15 |
| slash-only / D-Bus | 0.078 (0.077–0.082) | 0.082 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 201.99 / p95 207.72 / p99 211.54 / max 211.54 ms |
| ↳ rename | p50 202.68 / p95 203.43 / p99 216.36 / max 216.36 ms |
| ↳ delete | p50 202.25 / p95 203.34 / p99 212.50 / max 212.50 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.18 s after last write |
| burst: rename 10000 files (0.19 s) → consistent | 0.38 s after last rename |
| rename directory holding 10000 files → visible under new path | 229 ms |
| delete tree of 10000 files (0.16 s) → gone from index | 0.35 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 21.8 MiB / 23.6 MiB → 24.0 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 23.8 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.16 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 7.6 MiB (1.2 MiB / 6.4 MiB), 17.6 MiB |
| recovery: SIGKILL after 1.01 s (95200 files committed) → restart → complete | 0.27 s; 95200/95200 files ✔ |

