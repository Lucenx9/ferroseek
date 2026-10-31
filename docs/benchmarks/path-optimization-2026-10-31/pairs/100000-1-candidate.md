Ordinary word queries D-Bus: p50 3.12 / p95 3.58 / p99 3.86 / max 4.06 ms
Broad query "corpus/" D-Bus: p50 3.46 / p95 3.47 / p99 3.48 / max 3.48 ms
Broad query "corpus/ report" D-Bus: p50 8.01 / p95 9.02 / p99 9.35 / max 9.35 ms
Broad query "home/" D-Bus: p50 3.45 / p95 4.20 / p99 4.22 / max 4.22 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.316 (1.202–1.711) | 1.897 | 0.99 | 15/15 |
| exact-name / D-Bus | 1.333 (1.316–1.336) | 1.367 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.33 / p95 1.36 / p99 1.37 / max 1.37 ms | — | — | — |
| path-only / library | 0.874 (0.777–0.959) | 0.979 | 0.99 | 15/15 |
| path-only / D-Bus | 0.861 (0.843–0.868) | 0.879 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.86 / p95 0.87 / p99 0.88 / max 0.88 ms | — | — | — |
| path+name / library | 0.859 (0.752–0.971) | 1.039 | 1.00 | 15/15 |
| path+name / D-Bus | 0.899 (0.856–1.068) | 1.137 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.95 / p95 1.09 / p99 1.14 / max 1.14 ms | — | — | — |
| within+name / library | 0.089 (0.083–0.093) | 0.102 | 1.01 | 15/15 |
| path-miss / library | 0.395 (0.355–0.417) | 0.430 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.519 (0.489–0.541) | 0.565 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.52 / p95 0.55 / p99 0.56 / max 0.56 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.16 | 15/15 |
| slash-only / D-Bus | 0.067 (0.064–0.073) | 0.103 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.07 / p99 0.10 / max 0.10 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

