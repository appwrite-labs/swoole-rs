# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8701937` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,100 | 15,202 | 0.9275× | 0.91× | ✅ |
| echo-1k | 17,012 | 18,152 | 0.9372× | 0.92× | ✅ |
| json | 22,089 | 23,164 | 0.9536× | 0.94× | ✅ |
| service-health | 16,700 | 17,799 | 0.9383× | 0.93× | ✅ |
| websocket | 6,558 | 8,191 | 0.8006× | 0.81× | ✅ |
| timer | 211,426 | 314,118 | 0.6731× | 0.68× | ✅ |
| app | 125,576 | 135,896 | 0.9241× | 0.92× | ✅ |
| http2 | 19,479 | 20,343 | 0.9575× | 0.95× | ✅ |
| table | 197,390 | 201,681 | 0.9787× | 0.98× | ✅ |
| channel | 136,469 | 154,568 | 0.8829× | 0.89× | ✅ |
| task | 31,495 | 33,181 | 0.9492× | 0.94× | ✅ |
| mysql | 49,350 | 51,079 | 0.9662× | 0.97× | ✅ |

Geometric mean 0.9030×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
