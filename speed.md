# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7af2f12` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,793 | 15,198 | 0.9075× | 0.91× | ✅ |
| echo-1k | 16,690 | 18,145 | 0.9198× | 0.92× | ✅ |
| json | 21,743 | 23,164 | 0.9386× | 0.94× | ✅ |
| service-health | 16,392 | 17,799 | 0.9210× | 0.93× | ✅ |
| websocket | 6,600 | 8,191 | 0.8057× | 0.81× | ✅ |
| timer | 222,636 | 314,184 | 0.7086× | 0.72× | ✅ |
| app | 125,372 | 136,086 | 0.9213× | 0.92× | ✅ |
| http2 | 19,322 | 20,343 | 0.9498× | 0.95× | ✅ |
| table | 197,117 | 201,685 | 0.9774× | 0.99× | ✅ |
| channel | 136,484 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,131 | 33,178 | 0.9383× | 0.94× | ✅ |
| mysql | 49,064 | 51,082 | 0.9605× | 0.97× | ✅ |

Geometric mean 0.8995×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
