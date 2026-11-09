# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9d3ca05` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,778 | 15,211 | 0.9058× | 0.91× | ✅ |
| echo-1k | 16,638 | 18,153 | 0.9166× | 0.92× | ✅ |
| json | 21,733 | 23,161 | 0.9384× | 0.95× | ✅ |
| service-health | 16,385 | 17,795 | 0.9208× | 0.93× | ✅ |
| websocket | 6,599 | 8,188 | 0.8059× | 0.81× | ✅ |
| timer | 222,627 | 314,121 | 0.7087× | 0.73× | ✅ |
| app | 125,123 | 135,982 | 0.9201× | 0.93× | ✅ |
| http2 | 19,281 | 20,369 | 0.9466× | 0.95× | ✅ |
| table | 198,149 | 201,729 | 0.9823× | 0.99× | ✅ |
| channel | 136,519 | 155,155 | 0.8799× | 0.89× | ✅ |
| task | 31,106 | 33,187 | 0.9373× | 0.94× | ✅ |
| mysql | 49,052 | 51,065 | 0.9606× | 0.97× | ✅ |

Geometric mean 0.8987×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
