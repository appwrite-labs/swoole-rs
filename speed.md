# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `36839da` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,146 | 15,217 | 0.9296× | 0.91× | ✅ |
| echo-1k | 17,092 | 18,178 | 0.9403× | 0.92× | ✅ |
| json | 22,139 | 23,184 | 0.9549× | 0.94× | ✅ |
| service-health | 16,719 | 17,790 | 0.9398× | 0.93× | ✅ |
| websocket | 6,564 | 8,188 | 0.8016× | 0.81× | ✅ |
| timer | 207,884 | 313,524 | 0.6631× | 0.68× | ✅ |
| app | 125,395 | 136,008 | 0.9220× | 0.92× | ✅ |
| http2 | 19,505 | 20,387 | 0.9568× | 0.95× | ✅ |
| table | 196,984 | 203,101 | 0.9699× | 0.98× | ✅ |
| channel | 136,503 | 155,089 | 0.8802× | 0.89× | ✅ |
| task | 31,573 | 33,309 | 0.9479× | 0.94× | ✅ |
| mysql | 49,305 | 51,084 | 0.9652× | 0.97× | ✅ |

Geometric mean 0.9013×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
