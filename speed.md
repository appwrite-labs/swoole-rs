# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `16f2890` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,101 | 15,202 | 0.9276× | 0.91× | ✅ |
| echo-1k | 17,036 | 18,154 | 0.9385× | 0.92× | ✅ |
| json | 22,086 | 23,164 | 0.9535× | 0.94× | ✅ |
| service-health | 16,704 | 17,799 | 0.9385× | 0.93× | ✅ |
| websocket | 6,559 | 8,184 | 0.8014× | 0.81× | ✅ |
| timer | 211,166 | 313,218 | 0.6742× | 0.68× | ✅ |
| app | 125,520 | 136,016 | 0.9228× | 0.92× | ✅ |
| http2 | 19,490 | 20,339 | 0.9583× | 0.95× | ✅ |
| table | 197,394 | 201,681 | 0.9787× | 0.98× | ✅ |
| channel | 136,473 | 154,568 | 0.8829× | 0.89× | ✅ |
| task | 31,493 | 33,176 | 0.9493× | 0.94× | ✅ |
| mysql | 49,381 | 51,078 | 0.9668× | 0.97× | ✅ |

Geometric mean 0.9033×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
