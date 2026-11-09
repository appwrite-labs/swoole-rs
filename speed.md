# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cb3ae65` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,820 | 15,206 | 0.9088× | 0.91× | ✅ |
| echo-1k | 16,690 | 18,160 | 0.9191× | 0.92× | ✅ |
| json | 21,766 | 23,163 | 0.9397× | 0.95× | ✅ |
| service-health | 16,404 | 17,795 | 0.9219× | 0.93× | ✅ |
| websocket | 6,581 | 8,188 | 0.8037× | 0.81× | ✅ |
| timer | 226,735 | 311,826 | 0.7271× | 0.73× | ✅ |
| app | 125,129 | 135,744 | 0.9218× | 0.93× | ✅ |
| http2 | 19,278 | 20,341 | 0.9477× | 0.95× | ✅ |
| table | 197,701 | 201,681 | 0.9803× | 0.99× | ✅ |
| channel | 136,744 | 154,568 | 0.8847× | 0.89× | ✅ |
| task | 31,101 | 33,181 | 0.9373× | 0.94× | ✅ |
| mysql | 49,082 | 51,072 | 0.9610× | 0.97× | ✅ |

Geometric mean 0.9016×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
