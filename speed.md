# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `65e62af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,397 | 15,210 | 0.9465× | 0.94× | ✅ |
| echo-1k | 17,647 | 18,165 | 0.9714× | 0.97× | ✅ |
| json | 22,482 | 23,170 | 0.9703× | 0.97× | ✅ |
| service-health | 16,986 | 17,782 | 0.9553× | 0.95× | ✅ |
| websocket | 6,864 | 8,191 | 0.8380× | 0.84× | ✅ |
| timer | 202,747 | 313,718 | 0.6463× | 0.65× | ✅ |
| app | 126,780 | 135,912 | 0.9328× | 0.94× | ✅ |
| http2 | 20,339 | 20,464 | 0.9939× | 0.99× | ✅ |
| table | 196,334 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,690 | 154,664 | 0.9032× | 0.91× | ✅ |
| task | 32,470 | 33,268 | 0.9760× | 0.97× | ✅ |
| mysql | 50,018 | 51,061 | 0.9796× | 0.98× | ✅ |
| http-client | 34,215 | 35,827 | 0.9550× | 0.96× | ✅ |
| client | 18,901 | 19,766 | 0.9563× | 0.95× | ✅ |
| socket | 20,364 | 21,071 | 0.9664× | 0.97× | ✅ |
| gzip | 196,518 | 197,083 | 0.9971× | 1.0× | ✅ |
| websocket-deflate | 181,645 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,228 | 0.5730× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,253 | 18,703 | 1.0294× | 1.03× | ➖ |
| https | 109,616 | 108,813 | 1.0074× | 1.01× | ➖ |
| wss | 105,056 | 104,808 | 1.0024× | 1.0× | ❌ |
| atomic | 50,203 | 51,325 | 0.9781× | 0.98× | ✅ |
| process-mode | 16,657 | 16,962 | 0.9820× | 0.98× | ✅ |
| http2-concurrent | 378,254 | 409,072 | 0.9247× | 0.92× | ✅ |
| websocket-64k | 156,307 | 159,702 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 12,836 | 16,335 | 0.7858× | 0.87× | ✅ |
| upload | 674,244 | 1,322,381 | 0.5099× | 0.51× | ✅ |
| static | 35,165 | 41,074 | 0.8561× | 0.86× | ✅ |
| pgsql | 23,707 | 24,595 | 0.9639× | 0.97× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 0.96× | ✅ |
| curl | 86,359 | 90,507 | 0.9542× | 0.96× | ✅ |
| dns | 231,533 | 233,381 | 0.9921× | 1.0× | ✅ |

Geometric mean 0.9182×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
