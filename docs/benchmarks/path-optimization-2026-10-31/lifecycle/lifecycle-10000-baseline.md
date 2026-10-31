### 10000 files (seed 42)

Corpus: 10000 files in 500 directories (+300 files in excluded `node_modules`/`.git` trees), generated in 0.2s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 0.12 s (82478 files/s) |
| ↳ CPU time (user+sys) | 0.09 s |
| ↳ peak RSS (VmHWM) | 11.7 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 5.4 MiB |
| ↳ indexed files / dirs / watched dirs | 9505 / 510 / 510 |
| initial index, max_cpu_percent=100: wall | 0.11 s (91565 files/s) |
| ↳ CPU time (user+sys) | 0.10 s |
| ↳ peak RSS (VmHWM) | 11.8 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 5.4 MiB |
| ↳ indexed files / dirs / watched dirs | 9505 / 510 / 510 |
| index size on disk: database / WAL | 2.4 MiB (236 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 8.5 MiB (2.1 MiB anon, 6.4 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 0.11 / p95 0.41 / p99 0.55 / max 0.72 ms |
| query latency, D-Bus `Match` end-to-end | p50 0.38 / p95 0.83 / p99 0.98 / max 1.31 ms |
| RSS after query burst | 11.5 MiB |
| relevance (eligible 194 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.391 (0.372–0.464) | 0.480 | 0.99 | 15/15 |
| exact-name / D-Bus | 0.429 (0.418–0.463) | 0.475 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.43 / p95 0.46 / p99 0.48 / max 0.48 ms | — | — | — |
| path-only / library | 0.335 (0.313–0.375) | 0.388 | 1.00 | 15/15 |
| path-only / D-Bus | 4.994 (4.971–5.012) | 5.067 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 4.99 / p95 5.05 / p99 5.07 / max 5.07 ms | — | — | — |
| path+name / library | 0.335 (0.319–0.369) | 0.377 | 1.00 | 15/15 |
| path+name / D-Bus | 0.721 (0.670–0.791) | 0.926 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.72 / p95 0.82 / p99 0.93 / max 0.93 ms | — | — | — |
| within+name / library | 0.056 (0.055–0.063) | 0.074 | 1.01 | 15/15 |
| path-miss / library | 0.205 (0.188–0.231) | 0.232 | 1.00 | 15/15 |
| path-miss / D-Bus | 4.942 (4.912–4.958) | 4.982 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 4.93 / p95 4.96 / p99 4.98 / max 4.98 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.063 (0.061–0.067) | 0.092 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.06 / p95 0.07 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 202.09 / p95 203.08 / p99 203.16 / max 203.16 ms |
| ↳ rename | p50 202.35 / p95 204.69 / p99 206.82 / max 206.82 ms |
| ↳ delete | p50 201.67 / p95 203.20 / p99 203.35 / max 203.35 ms |
| burst: create 10000 files (0.11 s to write) → all searchable | 0.17 s after last write |
| burst: rename 10000 files (0.20 s) → consistent | 0.33 s after last rename |
| rename directory holding 10000 files → visible under new path | 227 ms |
| delete tree of 10000 files (0.16 s) → gone from index | 0.34 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 17.0 MiB / 13.5 MiB → 16.4 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 17.7 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 0.02 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 7.3 MiB (1.0 MiB / 6.2 MiB), 8.9 MiB |
| recovery: SIGKILL after 0.09 s (9505 files committed) → restart → complete | 0.03 s; 9505/9505 files ✔ |

