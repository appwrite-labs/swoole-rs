# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `67dc31e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,972 | 15,214 | 0.9184× | 0.92× | ✅ |
| echo-1k | 17,054 | 18,165 | 0.9388× | 0.94× | ✅ |
| json | 22,109 | 23,161 | 0.9546× | 0.95× | ✅ |
| service-health | 16,545 | 17,777 | 0.9307× | 0.93× | ✅ |
| websocket | 6,617 | 8,188 | 0.8081× | 0.8× | ✅ |
| timer | 205,992 | 313,577 | 0.6569× | 0.65× | ✅ |
| app | 125,358 | 135,933 | 0.9222× | 0.93× | ✅ |
| http2 | 18,857 | 20,468 | 0.9213× | 0.91× | ✅ |
| table | 195,112 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 137,256 | 155,170 | 0.8846× | 0.88× | ✅ |
| task | 32,504 | 33,265 | 0.9771× | 0.96× | ✅ |
| mysql | 49,547 | 51,062 | 0.9703× | 0.97× | ✅ |
| http-client | 33,898 | 35,799 | 0.9469× | 0.94× | ✅ |
| client | 18,542 | 19,756 | 0.9386× | 0.94× | ✅ |
| socket | 19,685 | 21,073 | 0.9342× | 0.92× | ✅ |
| gzip | 194,169 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,070 | 181,560 | 0.9863× | 0.99× | ✅ |
| tcp-length | 4,703 | 4,765 | 0.9869× | 0.99× | ✅ |
| tcp-eof | 5,198 | 10,231 | 0.5080× | 0.51× | ✅ |
| udp | 7,492 | 7,986 | 0.9380× | 0.93× | ✅ |
| connect | 17,742 | 18,703 | 0.9486× | 0.94× | ✅ |
| https | 108,189 | 108,806 | 0.9943× | 1.0× | ✅ |
| wss | 103,808 | 104,794 | 0.9906× | 0.99× | ✅ |
| atomic | 49,775 | 51,316 | 0.9700× | 0.97× | ✅ |
| process-mode | 15,068 | 16,960 | 0.8885× | 0.88× | ✅ |
| http2-concurrent | 365,005 | 409,074 | 0.8923× | 0.88× | ✅ |
| websocket-64k | 89,942 | 159,695 | 0.5632× | 0.57× | ✅ |
| websocket-fragmented | 11,868 | 16,327 | 0.7269× | 0.73× | ✅ |
| upload | 413,334 | 1,322,387 | 0.3126× | 0.32× | ✅ |
| static | 27,369 | 41,016 | 0.6673× | 0.66× | ✅ |
| pgsql | 23,395 | 24,595 | 0.9512× | 0.95× | ✅ |
| sqlite | 23,572 | 25,221 | 0.9346× | 0.93× | ✅ |
| curl | 86,152 | 90,351 | 0.9535× | 0.94× | ✅ |
| dns | 231,472 | 232,826 | 0.9942× | 0.99× | ✅ |

Geometric mean 0.8581×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
