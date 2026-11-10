# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `13934e0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,862 | 15,202 | 0.9119× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,151 | 0.9231× | 0.92× | ✅ |
| json | 21,837 | 23,164 | 0.9427× | 0.94× | ✅ |
| service-health | 16,465 | 17,799 | 0.9251× | 0.93× | ✅ |
| websocket | 6,599 | 8,191 | 0.8056× | 0.81× | ✅ |
| timer | 211,233 | 314,298 | 0.6721× | 0.68× | ✅ |
| app | 125,506 | 135,956 | 0.9231× | 0.92× | ✅ |
| http2 | 19,300 | 20,342 | 0.9488× | 0.95× | ✅ |
| table | 197,187 | 201,685 | 0.9777× | 0.98× | ✅ |
| channel | 136,555 | 154,568 | 0.8835× | 0.89× | ✅ |
| task | 31,213 | 33,178 | 0.9408× | 0.94× | ✅ |
| mysql | 49,117 | 51,082 | 0.9615× | 0.97× | ✅ |

Geometric mean 0.8972×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
