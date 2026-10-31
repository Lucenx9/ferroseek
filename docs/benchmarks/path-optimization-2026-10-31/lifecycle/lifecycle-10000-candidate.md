### 10000 files (seed 42)

Corpus: 10000 files in 500 directories (+300 files in excluded `node_modules`/`.git` trees), generated in 0.2s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 0.11 s (91682 files/s) |
| ↳ CPU time (user+sys) | 0.10 s |
| ↳ peak RSS (VmHWM) | 11.8 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 5.1 MiB |
| ↳ indexed files / dirs / watched dirs | 9505 / 510 / 510 |
| initial index, max_cpu_percent=100: wall | 0.11 s (91965 files/s) |
| ↳ CPU time (user+sys) | 0.09 s |
| ↳ peak RSS (VmHWM) | 11.7 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 5.2 MiB |
| ↳ indexed files / dirs / watched dirs | 9505 / 510 / 510 |
| index size on disk: database / WAL | 2.4 MiB (238 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 8.5 MiB (2.2 MiB anon, 6.3 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.11 / p95 0.40 / p99 0.54 / max 0.85 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.33 / p95 0.70 / p99 0.92 / max 1.34 ms |
| RSS after query burst | 11.5 MiB |
| relevance (eligible 194 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.383 (0.347–0.453) | 0.458 | 1.00 | 15/15 |
| exact-name / D-Bus | 0.502 (0.481–0.544) | 0.582 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.50 / p95 0.57 / p99 0.58 / max 0.58 ms | — | — | — |
| path-only / library | 0.398 (0.346–0.447) | 0.517 | 0.99 | 15/15 |
| path-only / D-Bus | 0.485 (0.471–0.585) | 0.642 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.49 / p95 0.59 / p99 0.64 / max 0.64 ms | — | — | — |
| path+name / library | 0.361 (0.336–0.409) | 0.432 | 0.99 | 15/15 |
| path+name / D-Bus | 0.403 (0.399–0.444) | 0.456 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.41 / p95 0.44 / p99 0.46 / max 0.46 ms | — | — | — |
| within+name / library | 0.040 (0.039–0.045) | 0.052 | 1.01 | 15/15 |
| path-miss / library | 0.196 (0.194–0.204) | 0.211 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.294 (0.291–0.300) | 0.329 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.29 / p95 0.31 / p99 0.33 / max 0.33 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.08 | 15/15 |
| slash-only / D-Bus | 0.073 (0.072–0.075) | 0.078 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 202.09 / p95 203.24 / p99 203.45 / max 203.45 ms |
| ↳ rename | p50 202.20 / p95 205.55 / p99 207.84 / max 207.84 ms |
| ↳ delete | p50 201.74 / p95 202.87 / p99 203.03 / max 203.03 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.17 s after last write |
| burst: rename 10000 files (0.19 s) → consistent | 0.35 s after last rename |
| rename directory holding 10000 files → visible under new path | 224 ms |
| delete tree of 10000 files (0.16 s) → gone from index | 0.34 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 17.1 MiB / 13.5 MiB → 16.8 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 17.4 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.02 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 7.3 MiB (1.0 MiB / 6.4 MiB), 9.1 MiB |
| recovery: SIGKILL after 0.08 s (9505 files committed) → restart → complete | 0.03 s; 9505/9505 files ✔ |

