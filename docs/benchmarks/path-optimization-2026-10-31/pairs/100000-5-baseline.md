Ordinary word queries D-Bus: p50 3.20 / p95 3.60 / p99 3.88 / max 4.35 ms
Broad query "corpus/" D-Bus: p50 3.48 / p95 3.55 / p99 3.56 / max 3.56 ms
Broad query "corpus/ report" D-Bus: p50 10.69 / p95 10.80 / p99 11.03 / max 11.03 ms
Broad query "home/" D-Bus: p50 3.48 / p95 3.69 / p99 4.30 / max 4.30 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.214 (1.203–1.419) | 1.663 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.393 (1.332–1.646) | 1.676 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.42 / p95 1.66 / p99 1.68 / max 1.68 ms | — | — | — |
| path-only / library | 0.763 (0.753–0.882) | 0.955 | 1.00 | 15/15 |
| path-only / D-Bus | 51.923 (51.166–52.543) | 52.692 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 51.92 / p95 52.65 / p99 52.69 / max 52.69 ms | — | — | — |
| path+name / library | 0.758 (0.752–0.886) | 0.980 | 0.99 | 15/15 |
| path+name / D-Bus | 8.209 (8.027–9.033) | 9.326 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 8.21 / p95 9.03 / p99 9.33 / max 9.33 ms | — | — | — |
| within+name / library | 0.083 (0.077–0.092) | 0.105 | 1.01 | 15/15 |
| path-miss / library | 0.380 (0.349–0.418) | 0.431 | 1.00 | 15/15 |
| path-miss / D-Bus | 51.820 (51.179–52.124) | 52.476 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 51.76 / p95 52.37 / p99 52.48 / max 52.48 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.13 | 15/15 |
| slash-only / D-Bus | 0.075 (0.074–0.084) | 0.092 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

