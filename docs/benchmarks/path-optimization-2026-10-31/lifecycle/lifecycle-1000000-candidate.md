### 1000000 files (seed 42)

Corpus: 1000000 files in 50000 directories (+30000 files in excluded `node_modules`/`.git` trees), generated in 24.8s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 42.17 s (23715 files/s) |
| ↳ CPU time (user+sys) | 29.35 s |
| ↳ peak RSS (VmHWM) | 23.8 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 3132.4 MiB |
| ↳ indexed files / dirs / watched dirs | 953595 / 51000 / 51000 |
| initial index, max_cpu_percent=100: wall | 18.68 s (53522 files/s) |
| ↳ CPU time (user+sys) | 22.64 s |
| ↳ peak RSS (VmHWM) | 23.5 MiB |
| ↳ disk read / written (/proc/pid/io) | 21.9 MiB / 1602.9 MiB |
| ↳ indexed files / dirs / watched dirs | 953595 / 51000 / 51000 |
| index size on disk: database / WAL | 251.2 MiB (251 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 15.6 MiB (9.0 MiB anon, 6.6 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 2.29 / p95 24.80 / p99 28.48 / max 39.92 ms |
| query latency, D-Bus `Match` end-to-end | p50 2.48 / p95 24.80 / p99 27.48 / max 41.48 ms |
| RSS after query burst | 19.3 MiB |
| relevance (eligible 189 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.047 (4.002–5.006) | 5.037 | 1.00 | 15/15 |
| exact-name / D-Bus | 4.041 (4.025–4.401) | 4.950 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.04 / p95 4.54 / p99 4.95 / max 4.95 ms | — | — | — |
| path-only / library | 1.771 (1.748–2.057) | 2.299 | 1.00 | 15/15 |
| path-only / D-Bus | 1.852 (1.842–1.867) | 1.876 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 1.85 / p95 1.87 / p99 1.88 / max 1.88 ms | — | — | — |
| path+name / library | 1.757 (1.747–1.777) | 1.817 | 1.00 | 15/15 |
| path+name / D-Bus | 1.834 (1.832–1.853) | 1.875 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 1.84 / p95 1.86 / p99 1.88 / max 1.88 ms | — | — | — |
| within+name / library | 0.055 (0.052–0.064) | 0.077 | 1.00 | 15/15 |
| path-miss / library | 0.426 (0.401–0.476) | 0.500 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.535 (0.472–0.593) | 0.617 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.54 / p95 0.59 / p99 0.62 / max 0.62 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.09 | 15/15 |
| slash-only / D-Bus | 0.066 (0.065–0.070) | 0.071 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.07 / p99 0.07 / max 0.07 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 203.21 / p95 210.86 / p99 215.41 / max 215.41 ms |
| ↳ rename | p50 203.34 / p95 213.01 / p99 214.71 / max 214.71 ms |
| ↳ delete | p50 202.48 / p95 203.98 / p99 204.06 / max 204.06 ms |
| burst: create 10000 files (0.12 s to write) → all searchable | 0.19 s after last write |
| burst: rename 10000 files (0.20 s) → consistent | 0.51 s after last rename |
| rename directory holding 10000 files → visible under new path | 225 ms |
| delete tree of 10000 files (0.16 s) → gone from index | 0.37 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 25.2 MiB / 25.1 MiB → 25.8 MiB |
| bursts: disk read / written; inotify overflows | 1.7 MiB / 38.7 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 1.20 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 10.1 MiB (3.8 MiB / 6.4 MiB), 21.8 MiB |
| recovery: SIGKILL after 5.37 s (360363 files committed) → restart → complete | 13.81 s; 953595/953595 files ✔ |

