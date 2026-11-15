# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e04b617` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,985 | 15,213 | 0.9193× | 0.92× | ✅ |
| echo-1k | 17,081 | 18,162 | 0.9405× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,773 | 0.9319× | 0.93× | ✅ |
| websocket | 6,653 | 8,191 | 0.8122× | 0.8× | ✅ |
| timer | 205,956 | 313,518 | 0.6569× | 0.65× | ✅ |
| app | 125,419 | 135,875 | 0.9230× | 0.93× | ✅ |
| http2 | 18,705 | 20,465 | 0.9140× | 0.91× | ✅ |
| table | 195,578 | 201,865 | 0.9689× | 0.97× | ✅ |
| channel | 136,599 | 155,175 | 0.8803× | 0.88× | ✅ |
| task | 32,524 | 33,264 | 0.9778× | 0.96× | ✅ |
| mysql | 49,590 | 51,061 | 0.9712× | 0.97× | ✅ |
| http-client | 33,833 | 35,803 | 0.9450× | 0.94× | ✅ |
| client | 18,571 | 19,757 | 0.9400× | 0.94× | ✅ |
| socket | 19,691 | 21,073 | 0.9344× | 0.92× | ✅ |
| gzip | 194,184 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,110 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,728 | 4,765 | 0.9922× | 0.99× | ✅ |
| tcp-eof | 5,221 | 10,231 | 0.5103× | 0.51× | ✅ |
| udp | 7,545 | 7,987 | 0.9447× | 0.93× | ✅ |
| connect | 17,771 | 18,707 | 0.9500× | 0.94× | ✅ |
| https | 108,233 | 108,810 | 0.9947× | 1.0× | ✅ |
| wss | 103,878 | 104,797 | 0.9912× | 0.99× | ✅ |
| atomic | 49,738 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,112 | 16,956 | 0.8912× | 0.88× | ✅ |
| http2-concurrent | 362,379 | 409,075 | 0.8858× | 0.88× | ✅ |
| websocket-64k | 89,978 | 159,697 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,894 | 16,331 | 0.7283× | 0.73× | ✅ |
| upload | 413,473 | 1,322,390 | 0.3127× | 0.32× | ✅ |
| static | 27,268 | 40,987 | 0.6653× | 0.66× | ✅ |
| pgsql | 23,388 | 24,594 | 0.9509× | 0.95× | ✅ |
| sqlite | 23,558 | 25,221 | 0.9340× | 0.93× | ✅ |
| curl | 86,138 | 90,350 | 0.9534× | 0.94× | ✅ |
| dns | 231,669 | 233,112 | 0.9938× | 0.99× | ✅ |

Geometric mean 0.8584×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
