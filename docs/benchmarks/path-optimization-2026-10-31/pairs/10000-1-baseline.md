Ordinary word queries D-Bus: p50 0.77 / p95 1.02 / p99 1.28 / max 2.12 ms
Broad query "corpus/" D-Bus: p50 3.41 / p95 3.57 / p99 4.16 / max 4.16 ms
Broad query "corpus/ report" D-Bus: p50 1.44 / p95 1.83 / p99 1.83 / max 1.83 ms
Broad query "home/" D-Bus: p50 3.42 / p95 3.73 / p99 3.95 / max 3.95 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.379 (0.365–0.467) | 0.498 | 1.00 | 15/15 |
| exact-name / D-Bus | 0.478 (0.461–0.566) | 0.747 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.48 / p95 0.59 / p99 0.75 / max 0.75 ms | — | — | — |
| path-only / library | 0.365 (0.349–0.430) | 0.450 | 0.99 | 15/15 |
| path-only / D-Bus | 4.923 (4.848–4.991) | 5.695 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 4.92 / p95 5.19 / p99 5.69 / max 5.69 ms | — | — | — |
| path+name / library | 0.362 (0.328–0.434) | 0.444 | 0.99 | 15/15 |
| path+name / D-Bus | 0.621 (0.581–0.734) | 0.764 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.62 / p95 0.73 / p99 0.76 / max 0.76 ms | — | — | — |
| within+name / library | 0.075 (0.074–0.082) | 0.101 | 1.00 | 15/15 |
| path-miss / library | 0.237 (0.229–0.248) | 0.260 | 0.99 | 15/15 |
| path-miss / D-Bus | 4.861 (4.846–4.875) | 4.886 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 4.86 / p95 4.88 / p99 4.89 / max 4.89 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.09 | 15/15 |
| slash-only / D-Bus | 0.064 (0.063–0.068) | 0.078 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.06 / p95 0.07 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

