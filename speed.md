# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cbc6e56` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,111 | 15,195 | 0.9287× | 0.91× | ✅ |
| echo-1k | 17,037 | 18,149 | 0.9387× | 0.92× | ✅ |
| json | 22,093 | 23,161 | 0.9539× | 0.94× | ✅ |
| service-health | 16,701 | 17,796 | 0.9385× | 0.93× | ✅ |
| websocket | 6,559 | 8,191 | 0.8007× | 0.81× | ✅ |
| timer | 210,307 | 310,996 | 0.6762× | 0.68× | ✅ |
| app | 125,346 | 135,926 | 0.9222× | 0.92× | ✅ |
| http2 | 19,492 | 20,340 | 0.9583× | 0.95× | ✅ |
| table | 197,389 | 201,682 | 0.9787× | 0.98× | ✅ |
| channel | 136,472 | 154,565 | 0.8829× | 0.89× | ✅ |
| task | 31,503 | 33,179 | 0.9495× | 0.94× | ✅ |
| mysql | 49,389 | 51,078 | 0.9669× | 0.97× | ✅ |

Geometric mean 0.9036×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
