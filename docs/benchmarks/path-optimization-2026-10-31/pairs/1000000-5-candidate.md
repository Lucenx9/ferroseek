Ordinary word queries D-Bus: p50 3.48 / p95 3.96 / p99 4.41 / max 4.55 ms
Broad query "corpus/" D-Bus: p50 3.46 / p95 3.48 / p99 3.49 / max 3.49 ms
Broad query "corpus/ report" D-Bus: p50 9.19 / p95 9.79 / p99 10.61 / max 10.61 ms
Broad query "home/" D-Bus: p50 3.46 / p95 3.72 / p99 4.01 / max 4.01 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.438 (4.400–4.558) | 5.472 | 1.00 | 15/15 |
| exact-name / D-Bus | 4.584 (4.561–4.846) | 5.184 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.60 / p95 4.94 / p99 5.18 / max 5.18 ms | — | — | — |
| path-only / library | 1.960 (1.957–2.437) | 2.636 | 1.00 | 15/15 |
| path-only / D-Bus | 2.066 (2.040–2.160) | 2.369 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 2.06 / p95 2.16 / p99 2.37 / max 2.37 ms | — | — | — |
| path+name / library | 1.969 (1.953–2.072) | 2.477 | 1.00 | 15/15 |
| path+name / D-Bus | 2.069 (2.047–2.153) | 2.386 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 2.06 / p95 2.15 / p99 2.39 / max 2.39 ms | — | — | — |
| within+name / library | 0.080 (0.079–0.094) | 0.096 | 1.00 | 15/15 |
| path-miss / library | 0.511 (0.462–0.563) | 0.570 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.601 (0.530–0.623) | 0.633 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.60 / p95 0.63 / p99 0.63 / max 0.63 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.13 | 15/15 |
| slash-only / D-Bus | 0.076 (0.075–0.077) | 0.081 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

