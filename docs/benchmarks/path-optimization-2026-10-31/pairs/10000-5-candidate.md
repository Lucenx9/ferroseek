Ordinary word queries D-Bus: p50 0.56 / p95 0.72 / p99 0.81 / max 0.95 ms
Broad query "corpus/" D-Bus: p50 3.28 / p95 3.30 / p99 3.41 / max 3.41 ms
Broad query "corpus/ report" D-Bus: p50 1.02 / p95 1.04 / p99 1.04 / max 1.04 ms
Broad query "home/" D-Bus: p50 3.28 / p95 3.54 / p99 3.92 / max 3.92 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.410 (0.384–0.432) | 0.588 | 0.98 | 15/15 |
| exact-name / D-Bus | 0.539 (0.500–0.624) | 0.670 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.54 / p95 0.62 / p99 0.67 / max 0.67 ms | — | — | — |
| path-only / library | 0.348 (0.318–0.362) | 0.397 | 1.00 | 15/15 |
| path-only / D-Bus | 0.484 (0.454–0.500) | 0.510 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.48 / p95 0.50 / p99 0.51 / max 0.51 ms | — | — | — |
| path+name / library | 0.336 (0.305–0.380) | 0.418 | 1.00 | 15/15 |
| path+name / D-Bus | 0.466 (0.456–0.503) | 0.578 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.47 / p95 0.50 / p99 0.58 / max 0.58 ms | — | — | — |
| within+name / library | 0.052 (0.052–0.061) | 0.068 | 1.01 | 15/15 |
| path-miss / library | 0.224 (0.214–0.270) | 0.288 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.320 (0.312–0.353) | 0.370 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.32 / p95 0.35 / p99 0.37 / max 0.37 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.13 | 15/15 |
| slash-only / D-Bus | 0.078 (0.064–0.081) | 0.085 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

