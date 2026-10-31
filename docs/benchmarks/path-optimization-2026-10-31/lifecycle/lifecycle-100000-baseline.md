### 100000 files (seed 42)

Corpus: 100000 files in 5000 directories (+3000 files in excluded `node_modules`/`.git` trees), generated in 1.8s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 2.29 s (43585 files/s) |
| ↳ CPU time (user+sys) | 1.71 s |
| ↳ peak RSS (VmHWM) | 17.2 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 91.6 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| initial index, max_cpu_percent=100: wall | 1.13 s (88845 files/s) |
| ↳ CPU time (user+sys) | 1.13 s |
| ↳ peak RSS (VmHWM) | 20.5 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 79.0 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| index size on disk: database / WAL | 25.1 MiB (251 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 9.2 MiB (2.8 MiB anon, 6.4 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.46 / p95 2.98 / p99 3.23 / max 4.75 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.79 / p95 3.18 / p99 3.72 / max 5.01 ms |
| RSS after query burst | 13.2 MiB |
| relevance (eligible 187 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.121 (1.081–1.472) | 1.602 | 0.99 | 15/15 |
| exact-name / D-Bus | 1.179 (1.172–1.364) | 1.389 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.18 / p95 1.36 / p99 1.39 / max 1.39 ms | — | — | — |
| path-only / library | 0.655 (0.650–0.803) | 0.804 | 0.99 | 15/15 |
| path-only / D-Bus | 51.687 (51.651–52.817) | 53.227 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 51.77 / p95 53.00 / p99 53.23 / max 53.23 ms | — | — | — |
| path+name / library | 0.673 (0.649–0.770) | 0.863 | 0.98 | 15/15 |
| path+name / D-Bus | 8.118 (7.895–8.740) | 9.076 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 8.22 / p95 8.74 / p99 9.08 / max 9.08 ms | — | — | — |
| within+name / library | 0.043 (0.043–0.051) | 0.057 | 1.01 | 15/15 |
| path-miss / library | 0.238 (0.232–0.247) | 0.342 | 0.97 | 15/15 |
| path-miss / D-Bus | 53.008 (51.694–53.363) | 54.359 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 52.91 / p95 54.16 / p99 54.36 / max 54.36 ms | — | — | — |
| slash-only / library | 0.000 (0.000–0.000) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.068 (0.066–0.072) | 0.091 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 202.39 / p95 207.70 / p99 214.66 / max 214.66 ms |
| ↳ rename | p50 202.18 / p95 203.62 / p99 214.89 / max 214.89 ms |
| ↳ delete | p50 202.02 / p95 203.49 / p99 210.90 / max 210.90 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.18 s after last write |
| burst: rename 10000 files (0.19 s) → consistent | 0.37 s after last rename |
| rename directory holding 10000 files → visible under new path | 231 ms |
| delete tree of 10000 files (0.16 s) → gone from index | 0.36 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 22.1 MiB / 21.6 MiB → 23.0 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 23.7 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.14 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 7.4 MiB (1.2 MiB / 6.2 MiB), 21.1 MiB |
| recovery: SIGKILL after 1.04 s (94153 files committed) → restart → complete | 0.30 s; 95200/95200 files ✔ |

