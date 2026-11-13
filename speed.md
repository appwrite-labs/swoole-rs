# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b5152ff` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,878 | 15,213 | 0.9123× | 0.92× | ✅ |
| echo-1k | 16,985 | 18,165 | 0.9350× | 0.94× | ✅ |
| json | 21,977 | 23,161 | 0.9489× | 0.95× | ✅ |
| service-health | 16,469 | 17,777 | 0.9265× | 0.93× | ✅ |
| websocket | 6,495 | 8,188 | 0.7933× | 0.8× | ✅ |
| timer | 204,689 | 313,563 | 0.6528× | 0.65× | ✅ |
| app | 125,539 | 135,954 | 0.9234× | 0.93× | ✅ |
| http2 | 18,554 | 20,468 | 0.9065× | 0.91× | ✅ |
| table | 195,102 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 137,947 | 155,174 | 0.8890× | 0.89× | ✅ |
| task | 31,954 | 33,264 | 0.9606× | 0.96× | ✅ |
| mysql | 49,564 | 51,049 | 0.9709× | 0.97× | ✅ |
| http-client | 33,773 | 35,801 | 0.9434× | 0.94× | ✅ |
| client | 18,500 | 19,757 | 0.9364× | 0.94× | ✅ |
| socket | 19,638 | 21,072 | 0.9319× | 0.92× | ✅ |
| gzip | 194,075 | 197,079 | 0.9848× | 0.99× | ✅ |
| websocket-deflate | 178,947 | 181,560 | 0.9856× | 0.99× | ✅ |
| tcp-length | 4,714 | 4,763 | 0.9898× | 0.99× | ✅ |
| tcp-eof | 5,207 | 10,228 | 0.5091× | 0.51× | ✅ |
| udp | 7,379 | 7,987 | 0.9239× | 0.93× | ✅ |
| connect | 17,649 | 18,707 | 0.9435× | 0.94× | ✅ |
| https | 108,134 | 108,810 | 0.9938× | 1.0× | ✅ |
| wss | 103,720 | 104,803 | 0.9897× | 0.99× | ✅ |
| atomic | 49,649 | 51,312 | 0.9676× | 0.97× | ✅ |
| process-mode | 14,930 | 16,957 | 0.8805× | 0.88× | ✅ |
| http2-concurrent | 358,507 | 409,069 | 0.8764× | 0.88× | ✅ |
| websocket-64k | 89,838 | 159,694 | 0.5626× | 0.98× | ✅ |
| websocket-fragmented | 11,742 | 16,327 | 0.7192× | 0.78× | ✅ |
| upload | 412,318 | 1,322,388 | 0.3118× | 0.37× | ✅ |
| static | 27,011 | 40,987 | 0.6590× | 0.67× | ✅ |
| pgsql | 23,312 | 24,595 | 0.9478× | 0.95× | ✅ |
| sqlite | 23,492 | 25,236 | 0.9309× | 0.93× | ✅ |
| curl | 85,907 | 90,487 | 0.9494× | 0.94× | ✅ |
| dns | 231,083 | 232,802 | 0.9926× | 0.99× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
