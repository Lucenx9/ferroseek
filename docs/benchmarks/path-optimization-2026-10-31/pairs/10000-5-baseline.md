Ordinary word queries D-Bus: p50 0.79 / p95 1.00 / p99 1.09 / max 1.19 ms
Broad query "corpus/" D-Bus: p50 3.44 / p95 4.08 / p99 4.20 / max 4.20 ms
Broad query "corpus/ report" D-Bus: p50 1.46 / p95 1.61 / p99 1.63 / max 1.63 ms
Broad query "home/" D-Bus: p50 3.44 / p95 3.55 / p99 3.58 / max 3.58 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.149 (0.146–0.159) | 0.192 | 1.00 | 15/15 |
| exact-name / D-Bus | 0.287 (0.245–0.303) | 0.309 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.28 / p95 0.31 / p99 0.31 / max 0.31 ms | — | — | — |
| path-only / library | 0.130 (0.127–0.156) | 0.157 | 1.00 | 15/15 |
| path-only / D-Bus | 4.895 (4.855–5.084) | 5.184 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 4.91 / p95 5.14 / p99 5.18 / max 5.18 ms | — | — | — |
| path+name / library | 0.138 (0.128–0.159) | 0.170 | 1.00 | 15/15 |
| path+name / D-Bus | 0.564 (0.562–0.608) | 0.659 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.57 / p95 0.61 / p99 0.66 / max 0.66 ms | — | — | — |
| within+name / library | 0.048 (0.048–0.058) | 0.066 | 1.01 | 15/15 |
| path-miss / library | 0.058 (0.055–0.066) | 0.074 | 1.01 | 15/15 |
| path-miss / D-Bus | 4.866 (4.816–4.920) | 5.175 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 4.87 / p95 5.10 / p99 5.17 / max 5.17 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.066 (0.065–0.074) | 0.123 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.09 / p99 0.12 / max 0.12 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

