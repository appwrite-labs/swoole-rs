# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8c407fe` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,797 | 15,198 | 0.9078× | 0.91× | ✅ |
| echo-1k | 16,674 | 18,150 | 0.9187× | 0.92× | ✅ |
| json | 21,751 | 23,164 | 0.9390× | 0.94× | ✅ |
| service-health | 16,392 | 17,799 | 0.9210× | 0.93× | ✅ |
| websocket | 6,601 | 8,188 | 0.8062× | 0.81× | ✅ |
| timer | 222,644 | 311,635 | 0.7144× | 0.72× | ✅ |
| app | 124,978 | 135,940 | 0.9194× | 0.92× | ✅ |
| http2 | 19,279 | 20,339 | 0.9479× | 0.95× | ✅ |
| table | 197,110 | 201,681 | 0.9773× | 0.99× | ✅ |
| channel | 136,481 | 154,569 | 0.8830× | 0.89× | ✅ |
| task | 31,133 | 33,176 | 0.9384× | 0.94× | ✅ |
| mysql | 49,058 | 51,078 | 0.9605× | 0.97× | ✅ |

Geometric mean 0.8998×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
