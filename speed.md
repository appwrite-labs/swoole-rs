# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f361ba4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,789 | 15,206 | 0.9068× | 0.91× | ✅ |
| echo-1k | 16,654 | 18,152 | 0.9175× | 0.92× | ✅ |
| json | 21,737 | 23,163 | 0.9384× | 0.95× | ✅ |
| service-health | 16,373 | 17,791 | 0.9203× | 0.93× | ✅ |
| websocket | 6,606 | 8,188 | 0.8069× | 0.81× | ✅ |
| timer | 222,634 | 314,119 | 0.7088× | 0.73× | ✅ |
| app | 125,114 | 136,096 | 0.9193× | 0.93× | ✅ |
| http2 | 19,317 | 20,345 | 0.9495× | 0.95× | ✅ |
| table | 198,119 | 201,685 | 0.9823× | 0.99× | ✅ |
| channel | 136,460 | 154,568 | 0.8828× | 0.89× | ✅ |
| task | 31,100 | 33,183 | 0.9372× | 0.94× | ✅ |
| mysql | 49,047 | 51,070 | 0.9604× | 0.97× | ✅ |

Geometric mean 0.8993×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
