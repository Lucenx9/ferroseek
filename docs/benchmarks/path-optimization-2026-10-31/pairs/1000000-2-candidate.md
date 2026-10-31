Ordinary word queries D-Bus: p50 3.49 / p95 3.94 / p99 4.30 / max 4.57 ms
Broad query "corpus/" D-Bus: p50 3.50 / p95 3.52 / p99 3.52 / max 3.52 ms
Broad query "corpus/ report" D-Bus: p50 9.14 / p95 9.29 / p99 9.31 / max 9.31 ms
Broad query "home/" D-Bus: p50 3.41 / p95 3.45 / p99 3.45 / max 3.45 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.470 (4.434–4.630) | 5.102 | 1.00 | 15/15 |
| exact-name / D-Bus | 4.581 (4.569–4.685) | 4.771 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.60 / p95 4.68 / p99 4.77 / max 4.77 ms | — | — | — |
| path-only / library | 2.000 (1.988–2.087) | 2.323 | 1.00 | 15/15 |
| path-only / D-Bus | 2.100 (2.093–2.183) | 2.417 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 2.10 / p95 2.18 / p99 2.42 / max 2.42 ms | — | — | — |
| path+name / library | 1.988 (1.985–2.097) | 2.299 | 1.00 | 15/15 |
| path+name / D-Bus | 2.204 (2.200–2.719) | 2.938 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 2.22 / p95 2.72 / p99 2.94 / max 2.94 ms | — | — | — |
| within+name / library | 0.078 (0.077–0.084) | 0.093 | 1.01 | 15/15 |
| path-miss / library | 0.486 (0.458–0.550) | 0.562 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.612 (0.572–0.646) | 0.757 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.63 / p95 0.69 / p99 0.76 / max 0.76 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.14 | 15/15 |
| slash-only / D-Bus | 0.074 (0.074–0.075) | 0.079 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

