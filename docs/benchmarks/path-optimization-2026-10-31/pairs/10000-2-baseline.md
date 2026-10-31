Ordinary word queries D-Bus: p50 0.74 / p95 0.91 / p99 0.98 / max 1.02 ms
Broad query "corpus/" D-Bus: p50 3.43 / p95 4.15 / p99 4.22 / max 4.22 ms
Broad query "corpus/ report" D-Bus: p50 1.42 / p95 1.58 / p99 1.65 / max 1.65 ms
Broad query "home/" D-Bus: p50 3.45 / p95 3.61 / p99 3.61 / max 3.61 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.443 (0.397–0.509) | 0.534 | 0.99 | 15/15 |
| exact-name / D-Bus | 0.465 (0.448–0.522) | 0.529 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.46 / p95 0.52 / p99 0.53 / max 0.53 ms | — | — | — |
| path-only / library | 0.338 (0.334–0.359) | 0.361 | 0.99 | 15/15 |
| path-only / D-Bus | 4.852 (4.822–5.271) | 5.534 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 4.85 / p95 5.41 / p99 5.53 / max 5.53 ms | — | — | — |
| path+name / library | 0.389 (0.344–0.418) | 0.447 | 0.99 | 15/15 |
| path+name / D-Bus | 0.737 (0.662–0.842) | 0.853 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.73 / p95 0.84 / p99 0.85 / max 0.85 ms | — | — | — |
| within+name / library | 0.082 (0.081–0.089) | 0.100 | 1.00 | 15/15 |
| path-miss / library | 0.241 (0.237–0.274) | 0.356 | 0.99 | 15/15 |
| path-miss / D-Bus | 4.826 (4.819–5.642) | 5.779 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 4.83 / p95 5.64 / p99 5.78 / max 5.78 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.10 | 15/15 |
| slash-only / D-Bus | 0.062 (0.062–0.067) | 0.068 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.06 / p95 0.07 / p99 0.07 / max 0.07 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

