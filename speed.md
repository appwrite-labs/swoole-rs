# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `51b46eb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,104 | 15,241 | 0.9254× | 0.91× | ✅ |
| echo-1k | 17,018 | 18,189 | 0.9356× | 0.92× | ✅ |
| json | 22,088 | 23,203 | 0.9519× | 0.94× | ✅ |
| service-health | 16,708 | 17,835 | 0.9368× | 0.93× | ✅ |
| websocket | 6,562 | 8,227 | 0.7976× | 0.81× | ✅ |
| timer | 211,413 | 314,308 | 0.6726× | 0.68× | ✅ |
| app | 125,314 | 136,217 | 0.9200× | 0.92× | ✅ |
| http2 | 19,486 | 20,381 | 0.9561× | 0.95× | ✅ |
| table | 197,394 | 201,724 | 0.9785× | 0.98× | ✅ |
| channel | 136,472 | 154,646 | 0.8825× | 0.89× | ✅ |
| task | 31,490 | 33,184 | 0.9490× | 0.94× | ✅ |
| mysql | 49,369 | 51,112 | 0.9659× | 0.97× | ✅ |

Geometric mean 0.9016×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
