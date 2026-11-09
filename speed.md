# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5502290` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,782 | 15,203 | 0.9065× | 0.91× | ✅ |
| echo-1k | 16,636 | 18,146 | 0.9168× | 0.92× | ✅ |
| json | 21,731 | 23,163 | 0.9382× | 0.95× | ✅ |
| service-health | 16,380 | 17,791 | 0.9207× | 0.93× | ✅ |
| websocket | 6,602 | 8,188 | 0.8063× | 0.81× | ✅ |
| timer | 222,774 | 310,976 | 0.7164× | 0.73× | ✅ |
| app | 125,146 | 136,062 | 0.9198× | 0.93× | ✅ |
| http2 | 19,286 | 20,342 | 0.9481× | 0.95× | ✅ |
| table | 198,115 | 204,110 | 0.9706× | 0.99× | ✅ |
| channel | 136,462 | 154,568 | 0.8829× | 0.89× | ✅ |
| task | 31,099 | 33,178 | 0.9373× | 0.94× | ✅ |
| mysql | 49,048 | 51,069 | 0.9604× | 0.97× | ✅ |

Geometric mean 0.8991×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
