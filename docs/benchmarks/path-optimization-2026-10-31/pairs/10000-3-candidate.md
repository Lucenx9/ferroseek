Ordinary word queries D-Bus: p50 0.63 / p95 0.80 / p99 0.88 / max 0.91 ms
Broad query "corpus/" D-Bus: p50 3.37 / p95 3.65 / p99 4.23 / max 4.23 ms
Broad query "corpus/ report" D-Bus: p50 1.10 / p95 1.13 / p99 1.14 / max 1.14 ms
Broad query "home/" D-Bus: p50 3.33 / p95 3.73 / p99 3.77 / max 3.77 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.503 (0.469–0.587) | 0.639 | 1.00 | 15/15 |
| exact-name / D-Bus | 0.549 (0.534–0.696) | 0.716 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.55 / p95 0.70 / p99 0.72 / max 0.72 ms | — | — | — |
| path-only / library | 0.409 (0.403–0.430) | 0.531 | 1.00 | 15/15 |
| path-only / D-Bus | 0.600 (0.566–0.607) | 0.734 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.60 / p95 0.69 / p99 0.73 / max 0.73 ms | — | — | — |
| path+name / library | 0.472 (0.431–0.584) | 0.686 | 0.99 | 15/15 |
| path+name / D-Bus | 0.541 (0.492–0.583) | 0.587 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.54 / p95 0.59 / p99 0.59 / max 0.59 ms | — | — | — |
| within+name / library | 0.073 (0.073–0.090) | 0.115 | 0.98 | 15/15 |
| path-miss / library | 0.308 (0.285–0.354) | 0.368 | 0.99 | 15/15 |
| path-miss / D-Bus | 0.403 (0.377–0.411) | 0.445 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.40 / p95 0.42 / p99 0.44 / max 0.44 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.16 | 15/15 |
| slash-only / D-Bus | 0.071 (0.066–0.099) | 0.119 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.12 / p99 0.12 / max 0.12 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

