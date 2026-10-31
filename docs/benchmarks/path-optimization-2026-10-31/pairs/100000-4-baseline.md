Ordinary word queries D-Bus: p50 3.23 / p95 3.67 / p99 4.07 / max 4.22 ms
Broad query "corpus/" D-Bus: p50 3.49 / p95 3.54 / p99 3.59 / max 3.59 ms
Broad query "corpus/ report" D-Bus: p50 10.73 / p95 11.66 / p99 12.24 / max 12.24 ms
Broad query "home/" D-Bus: p50 3.47 / p95 3.50 / p99 3.52 / max 3.52 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.177 (1.136–1.325) | 1.587 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.265 (1.260–1.280) | 1.307 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.27 / p95 1.29 / p99 1.31 / max 1.31 ms | — | — | — |
| path-only / library | 0.735 (0.692–0.876) | 0.894 | 1.00 | 15/15 |
| path-only / D-Bus | 52.422 (51.839–52.895) | 54.990 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 52.32 / p95 54.49 / p99 54.99 / max 54.99 ms | — | — | — |
| path+name / library | 0.742 (0.696–0.806) | 1.012 | 0.98 | 15/15 |
| path+name / D-Bus | 8.163 (8.087–8.230) | 9.062 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 8.11 / p95 8.40 / p99 9.06 / max 9.06 ms | — | — | — |
| within+name / library | 0.081 (0.074–0.085) | 0.095 | 1.00 | 15/15 |
| path-miss / library | 0.334 (0.301–0.359) | 0.399 | 0.99 | 15/15 |
| path-miss / D-Bus | 51.227 (51.141–51.691) | 52.710 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 51.24 / p95 52.62 / p99 52.71 / max 52.71 ms | — | — | — |
| slash-only / library | 0.000 (0.000–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.085 (0.084–0.095) | 0.097 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.09 / p95 0.10 / p99 0.10 / max 0.10 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

