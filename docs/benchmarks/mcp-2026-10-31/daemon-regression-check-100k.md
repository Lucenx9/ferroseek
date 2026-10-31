### 100000 files (seed 42)

Corpus: 100000 files in 5000 directories (+3000 files in excluded `node_modules`/`.git` trees), generated in 1.8s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 1.10 s (91174 files/s) |
| ↳ CPU time (user+sys) | 1.10 s |
| ↳ peak RSS (VmHWM) | 21.5 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 76.5 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| initial index, max_cpu_percent=100: wall | 1.09 s (91708 files/s) |
| ↳ CPU time (user+sys) | 1.10 s |
| ↳ peak RSS (VmHWM) | 22.6 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 72.5 MiB |
| ↳ indexed files / dirs / watched dirs | 95200 / 5100 / 5100 |
| index size on disk: database / WAL | 25.3 MiB (252 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 9.0 MiB (2.7 MiB anon, 6.4 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.45 / p95 2.93 / p99 3.17 / max 4.93 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.66 / p95 2.99 / p99 3.36 / max 5.05 ms |
| RSS after query burst | 12.6 MiB |
| relevance (eligible 187 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.093 (1.083–1.552) | 1.573 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.195 (1.181–1.387) | 1.410 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.19 / p95 1.39 / p99 1.41 / max 1.41 ms | — | — | — |
| path-only / library | 0.656 (0.645–0.760) | 0.795 | 1.00 | 15/15 |
| path-only / D-Bus | 0.793 (0.745–0.881) | 0.974 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.79 / p95 0.88 / p99 0.97 / max 0.97 ms | — | — | — |
| path+name / library | 0.668 (0.649–0.841) | 0.847 | 1.00 | 15/15 |
| path+name / D-Bus | 0.786 (0.741–0.884) | 0.895 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.79 / p95 0.89 / p99 0.90 / max 0.90 ms | — | — | — |
| within+name / library | 0.050 (0.049–0.056) | 0.065 | 1.01 | 15/15 |
| path-miss / library | 0.268 (0.254–0.281) | 0.293 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.374 (0.340–0.431) | 0.452 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.37 / p95 0.43 / p99 0.45 / max 0.45 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.14 | 15/15 |
| slash-only / D-Bus | 0.065 (0.063–0.068) | 0.085 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.06 / p95 0.07 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 202.41 / p95 207.21 / p99 213.10 / max 213.10 ms |
| ↳ rename | p50 202.56 / p95 203.79 / p99 216.18 / max 216.18 ms |
| ↳ delete | p50 201.72 / p95 203.65 / p99 210.68 / max 210.68 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.18 s after last write |
| burst: rename 10000 files (0.20 s) → consistent | 0.37 s after last rename |
| rename directory holding 10000 files → visible under new path | 229 ms |
| delete tree of 10000 files (0.18 s) → gone from index | 0.35 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 19.5 MiB / 22.6 MiB → 22.6 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 25.7 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.16 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 7.4 MiB (1.2 MiB / 6.2 MiB), 17.4 MiB |
| recovery: SIGKILL after 1.00 s (95200 files committed) → restart → complete | 0.28 s; 95200/95200 files ✔ |

