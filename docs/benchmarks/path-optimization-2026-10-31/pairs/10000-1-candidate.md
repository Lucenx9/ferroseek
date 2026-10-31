Ordinary word queries D-Bus: p50 0.58 / p95 0.72 / p99 0.83 / max 0.87 ms
Broad query "corpus/" D-Bus: p50 3.30 / p95 3.46 / p99 3.56 / max 3.56 ms
Broad query "corpus/ report" D-Bus: p50 1.03 / p95 1.25 / p99 1.36 / max 1.36 ms
Broad query "home/" D-Bus: p50 3.28 / p95 3.31 / p99 3.31 / max 3.31 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.366 (0.339–0.420) | 0.614 | 1.00 | 15/15 |
| exact-name / D-Bus | 0.462 (0.420–0.582) | 0.615 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.46 / p95 0.58 / p99 0.61 / max 0.61 ms | — | — | — |
| path-only / library | 0.372 (0.351–0.468) | 0.722 | 1.00 | 15/15 |
| path-only / D-Bus | 0.450 (0.428–0.458) | 0.563 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.45 / p95 0.50 / p99 0.56 / max 0.56 ms | — | — | — |
| path+name / library | 0.356 (0.323–0.369) | 0.375 | 1.00 | 15/15 |
| path+name / D-Bus | 0.475 (0.449–0.553) | 0.619 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.47 / p95 0.57 / p99 0.62 / max 0.62 ms | — | — | — |
| within+name / library | 0.068 (0.068–0.096) | 0.104 | 0.98 | 15/15 |
| path-miss / library | 0.189 (0.184–0.215) | 0.222 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.345 (0.325–0.351) | 0.378 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.34 / p95 0.35 / p99 0.38 / max 0.38 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.11 | 15/15 |
| slash-only / D-Bus | 0.082 (0.080–0.090) | 0.096 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.09 / p99 0.10 / max 0.10 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

