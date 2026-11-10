# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d1e6e73` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,110 | 15,202 | 0.9282× | 0.91× | ✅ |
| echo-1k | 17,007 | 18,150 | 0.9370× | 0.92× | ✅ |
| json | 22,096 | 23,164 | 0.9539× | 0.94× | ✅ |
| service-health | 16,712 | 17,796 | 0.9391× | 0.93× | ✅ |
| websocket | 6,558 | 8,188 | 0.8009× | 0.81× | ✅ |
| timer | 211,094 | 313,283 | 0.6738× | 0.68× | ✅ |
| app | 125,374 | 136,102 | 0.9212× | 0.92× | ✅ |
| http2 | 19,455 | 20,343 | 0.9564× | 0.95× | ✅ |
| table | 197,390 | 201,681 | 0.9787× | 0.98× | ✅ |
| channel | 136,476 | 154,568 | 0.8829× | 0.89× | ✅ |
| task | 31,495 | 33,176 | 0.9493× | 0.94× | ✅ |
| mysql | 49,368 | 51,078 | 0.9665× | 0.97× | ✅ |

Geometric mean 0.9029×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
