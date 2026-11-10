# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `01e6a22` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,109 | 15,202 | 0.9281× | 0.91× | ✅ |
| echo-1k | 17,060 | 18,166 | 0.9391× | 0.92× | ✅ |
| json | 22,098 | 23,164 | 0.9540× | 0.94× | ✅ |
| service-health | 16,702 | 17,799 | 0.9383× | 0.93× | ✅ |
| websocket | 6,559 | 8,184 | 0.8014× | 0.81× | ✅ |
| timer | 207,873 | 314,211 | 0.6616× | 0.68× | ✅ |
| app | 125,123 | 136,054 | 0.9197× | 0.92× | ✅ |
| http2 | 19,498 | 20,339 | 0.9586× | 0.95× | ✅ |
| table | 197,395 | 203,089 | 0.9720× | 0.98× | ✅ |
| channel | 136,477 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,488 | 33,180 | 0.9490× | 0.94× | ✅ |
| mysql | 49,323 | 51,079 | 0.9656× | 0.97× | ✅ |

Geometric mean 0.9012×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
