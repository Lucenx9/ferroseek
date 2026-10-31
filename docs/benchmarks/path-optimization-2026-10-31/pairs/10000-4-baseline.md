Ordinary word queries D-Bus: p50 0.79 / p95 1.00 / p99 1.09 / max 1.15 ms
Broad query "corpus/" D-Bus: p50 3.41 / p95 3.85 / p99 3.86 / max 3.86 ms
Broad query "corpus/ report" D-Bus: p50 1.47 / p95 1.67 / p99 1.81 / max 1.81 ms
Broad query "home/" D-Bus: p50 3.41 / p95 4.03 / p99 4.07 / max 4.07 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.558 (0.529–0.608) | 0.869 | 0.96 | 15/15 |
| exact-name / D-Bus | 0.591 (0.568–0.697) | 0.765 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.59 / p95 0.70 / p99 0.77 / max 0.77 ms | — | — | — |
| path-only / library | 0.498 (0.441–0.548) | 0.598 | 1.00 | 15/15 |
| path-only / D-Bus | 4.871 (4.825–5.106) | 5.245 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 4.87 / p95 5.20 / p99 5.24 / max 5.24 ms | — | — | — |
| path+name / library | 0.432 (0.430–0.454) | 0.479 | 0.99 | 15/15 |
| path+name / D-Bus | 0.634 (0.630–0.654) | 0.679 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.64 / p95 0.66 / p99 0.68 / max 0.68 ms | — | — | — |
| within+name / library | 0.060 (0.059–0.066) | 0.073 | 1.01 | 15/15 |
| path-miss / library | 0.282 (0.279–0.374) | 0.377 | 0.99 | 15/15 |
| path-miss / D-Bus | 4.877 (4.812–5.008) | 5.694 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 4.87 / p95 5.38 / p99 5.69 / max 5.69 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.06 | 15/15 |
| slash-only / D-Bus | 0.086 (0.080–0.088) | 0.114 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.09 / p95 0.09 / p99 0.11 / max 0.11 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

