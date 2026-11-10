# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5779c2d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,794 | 15,202 | 0.9074× | 0.91× | ✅ |
| echo-1k | 16,670 | 18,160 | 0.9180× | 0.92× | ✅ |
| json | 21,754 | 23,164 | 0.9391× | 0.94× | ✅ |
| service-health | 16,396 | 17,799 | 0.9212× | 0.93× | ✅ |
| websocket | 6,603 | 8,188 | 0.8065× | 0.81× | ✅ |
| timer | 222,639 | 314,487 | 0.7079× | 0.72× | ✅ |
| app | 125,421 | 136,078 | 0.9217× | 0.92× | ✅ |
| http2 | 19,278 | 20,339 | 0.9478× | 0.95× | ✅ |
| table | 197,114 | 203,089 | 0.9706× | 0.98× | ✅ |
| channel | 136,485 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,135 | 33,177 | 0.9384× | 0.94× | ✅ |
| mysql | 49,055 | 51,079 | 0.9604× | 0.97× | ✅ |

Geometric mean 0.8987×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
