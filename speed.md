# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be4e4ca` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,147 | 15,221 | 0.9294× | 0.91× | ✅ |
| echo-1k | 17,086 | 18,167 | 0.9405× | 0.92× | ✅ |
| json | 22,131 | 23,180 | 0.9548× | 0.94× | ✅ |
| service-health | 16,715 | 17,790 | 0.9396× | 0.93× | ✅ |
| websocket | 6,563 | 8,191 | 0.8012× | 0.81× | ✅ |
| timer | 207,907 | 313,437 | 0.6633× | 0.68× | ✅ |
| app | 125,367 | 135,883 | 0.9226× | 0.92× | ✅ |
| http2 | 19,517 | 20,387 | 0.9573× | 0.95× | ✅ |
| table | 196,977 | 201,693 | 0.9766× | 0.98× | ✅ |
| channel | 136,497 | 155,084 | 0.8802× | 0.89× | ✅ |
| task | 31,575 | 33,315 | 0.9478× | 0.94× | ✅ |
| mysql | 49,284 | 51,081 | 0.9648× | 0.97× | ✅ |

Geometric mean 0.9018×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
