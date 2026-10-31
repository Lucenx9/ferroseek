### 1000000 files (seed 42)

Corpus: 1000000 files in 50000 directories (+30000 files in excluded `node_modules`/`.git` trees), generated in 23.8s.

| metric | value |
|---|---|
| initial index, max_cpu_percent=50: wall | 38.10 s (26248 files/s) |
| ↳ CPU time (user+sys) | 27.42 s |
| ↳ peak RSS (VmHWM) | 21.7 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 1714.2 MiB |
| ↳ indexed files / dirs / watched dirs | 953595 / 51000 / 51000 |
| initial index, max_cpu_percent=100: wall | 20.17 s (49580 files/s) |
| ↳ CPU time (user+sys) | 23.75 s |
| ↳ peak RSS (VmHWM) | 21.0 MiB |
| ↳ disk read / written (/proc/pid/io) | 0.0 MiB / 1980.5 MiB |
| ↳ indexed files / dirs / watched dirs | 953595 / 51000 / 51000 |
| index size on disk: database / WAL | 250.5 MiB (250 B/entry) / 0.0 MiB |
| idle RSS (anon + file-backed) | 11.6 MiB (4.9 MiB anon, 6.7 MiB file) |
| idle CPU over 30 s | 0.000 % (0.00 s) |
| idle context switches (all threads) | 0.00 /s |
| idle threads | 4 |
| query latency, library (limit 15, 390 queries) | p50 2.38 / p95 25.44 / p99 29.50 / max 39.97 ms |
| query latency, D-Bus `Match` end-to-end | p50 2.57 / p95 25.07 / p99 27.54 / max 39.93 ms |
| RSS after query burst | 15.9 MiB |
| relevance (eligible 189 of sample 200): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | 100.0 % / 100.0 % / 100.0 % |

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.665 (4.586–5.583) | 5.735 | 1.00 | 15/15 |
| exact-name / D-Bus | 4.605 (4.568–5.429) | 5.484 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.61 / p95 5.43 / p99 5.48 / max 5.48 ms | — | — | — |
| path-only / library | 2.087 (2.064–2.131) | 2.349 | 1.00 | 15/15 |
| path-only / D-Bus | 540.158 (535.854–566.244) | 578.031 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 540.33 / p95 566.24 / p99 578.03 / max 578.03 ms | — | — | — |
| path+name / library | 2.098 (2.046–2.605) | 2.620 | 1.00 | 15/15 |
| path+name / D-Bus | 111.975 (110.393–113.442) | 115.358 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 111.09 / p95 113.97 / p99 115.36 / max 115.36 ms | — | — | — |
| within+name / library | 0.055 (0.053–0.063) | 0.074 | 1.01 | 15/15 |
| path-miss / library | 0.436 (0.416–0.545) | 0.582 | 1.00 | 15/15 |
| path-miss / D-Bus | 538.929 (536.658–540.033) | 549.144 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 538.24 / p95 540.65 / p99 549.14 / max 549.14 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.10 | 15/15 |
| slash-only / D-Bus | 0.076 (0.074–0.078) | 0.093 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.08 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|
| change → visible in `Match`: create (debounce 200 ms) | p50 202.95 / p95 204.14 / p99 214.95 / max 214.95 ms |
| ↳ rename | p50 203.65 / p95 214.93 / p99 221.42 / max 221.42 ms |
| ↳ delete | p50 202.28 / p95 203.96 / p99 203.98 / max 203.98 ms |
| burst: create 10000 files (0.12 s to write) → all searchable | 0.18 s after last write |
| burst: rename 10000 files (0.20 s) → consistent | 0.62 s after last rename |
| rename directory holding 10000 files → visible under new path | 225 ms |
| delete tree of 10000 files (0.17 s) → gone from index | 0.38 s after deletion |
| bursts: peak RSS sampled / VmHWM before→after | 25.1 MiB / 26.3 MiB → 26.3 MiB |
| bursts: disk read / written; inotify overflows | 0.0 MiB / 50.1 MiB; 0 |
| restart on unchanged tree → idle (quick reconcile + watches) | 1.03 s |
| steady-state idle after restart: RSS (anon / file), VmHWM | 9.7 MiB (3.5 MiB / 6.2 MiB), 21.8 MiB |
| recovery: SIGKILL after 5.37 s (341651 files committed) → restart → complete | 14.59 s; 953595/953595 files ✔ |

