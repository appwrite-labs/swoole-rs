# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `33272e4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,793 | 15,198 | 0.9075× | 0.91× | ✅ |
| echo-1k | 16,670 | 18,146 | 0.9186× | 0.92× | ✅ |
| json | 21,743 | 23,161 | 0.9388× | 0.94× | ✅ |
| service-health | 16,391 | 17,799 | 0.9209× | 0.93× | ✅ |
| websocket | 6,600 | 8,188 | 0.8061× | 0.81× | ✅ |
| timer | 222,642 | 313,307 | 0.7106× | 0.72× | ✅ |
| app | 125,506 | 135,915 | 0.9234× | 0.92× | ✅ |
| http2 | 19,286 | 20,339 | 0.9482× | 0.95× | ✅ |
| table | 197,114 | 201,681 | 0.9774× | 0.99× | ✅ |
| channel | 136,485 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,132 | 33,178 | 0.9383× | 0.94× | ✅ |
| mysql | 49,061 | 51,077 | 0.9605× | 0.97× | ✅ |

Geometric mean 0.8997×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
