# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3bf95e4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,862 | 15,199 | 0.9121× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,145 | 0.9233× | 0.92× | ✅ |
| json | 21,837 | 23,161 | 0.9428× | 0.94× | ✅ |
| service-health | 16,465 | 17,799 | 0.9251× | 0.93× | ✅ |
| websocket | 6,600 | 8,188 | 0.8061× | 0.81× | ✅ |
| timer | 210,429 | 311,139 | 0.6763× | 0.68× | ✅ |
| app | 125,246 | 136,060 | 0.9205× | 0.92× | ✅ |
| http2 | 19,301 | 20,340 | 0.9489× | 0.95× | ✅ |
| table | 197,184 | 201,682 | 0.9777× | 0.98× | ✅ |
| channel | 136,554 | 154,565 | 0.8835× | 0.89× | ✅ |
| task | 31,209 | 33,180 | 0.9406× | 0.94× | ✅ |
| mysql | 49,116 | 51,078 | 0.9616× | 0.97× | ✅ |

Geometric mean 0.8975×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
