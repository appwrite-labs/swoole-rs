# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `dde469d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,977 | 15,214 | 0.9187× | 0.92× | ✅ |
| echo-1k | 17,063 | 18,165 | 0.9393× | 0.94× | ✅ |
| json | 22,113 | 23,161 | 0.9547× | 0.95× | ✅ |
| service-health | 16,545 | 17,777 | 0.9307× | 0.93× | ✅ |
| websocket | 6,616 | 8,188 | 0.8081× | 0.8× | ✅ |
| timer | 206,151 | 313,558 | 0.6575× | 0.65× | ✅ |
| app | 125,336 | 135,876 | 0.9224× | 0.93× | ✅ |
| http2 | 18,861 | 20,464 | 0.9217× | 0.91× | ✅ |
| table | 195,108 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 136,740 | 155,170 | 0.8812× | 0.88× | ✅ |
| task | 32,499 | 33,262 | 0.9771× | 0.96× | ✅ |
| mysql | 49,553 | 51,061 | 0.9705× | 0.97× | ✅ |
| http-client | 33,880 | 35,803 | 0.9463× | 0.94× | ✅ |
| client | 18,543 | 19,758 | 0.9385× | 0.94× | ✅ |
| socket | 19,676 | 21,069 | 0.9339× | 0.92× | ✅ |
| gzip | 194,190 | 197,083 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,074 | 181,572 | 0.9862× | 0.99× | ✅ |
| tcp-length | 4,703 | 4,765 | 0.9869× | 0.99× | ✅ |
| tcp-eof | 5,198 | 10,231 | 0.5080× | 0.51× | ✅ |
| udp | 7,489 | 7,987 | 0.9377× | 0.93× | ✅ |
| connect | 17,741 | 18,707 | 0.9484× | 0.94× | ✅ |
| https | 108,193 | 108,810 | 0.9943× | 1.0× | ✅ |
| wss | 103,805 | 104,797 | 0.9905× | 0.99× | ✅ |
| atomic | 49,775 | 51,316 | 0.9700× | 0.97× | ✅ |
| process-mode | 15,091 | 16,956 | 0.8900× | 0.88× | ✅ |
| http2-concurrent | 365,225 | 409,073 | 0.8928× | 0.88× | ✅ |
| websocket-64k | 89,940 | 159,698 | 0.5632× | 0.57× | ✅ |
| websocket-fragmented | 11,868 | 16,327 | 0.7269× | 0.73× | ✅ |
| upload | 413,478 | 1,322,389 | 0.3127× | 0.32× | ✅ |
| static | 27,390 | 41,016 | 0.6678× | 0.66× | ✅ |
| pgsql | 23,374 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,549 | 25,236 | 0.9332× | 0.93× | ✅ |
| curl | 86,204 | 90,357 | 0.9540× | 0.94× | ✅ |
| dns | 231,747 | 232,892 | 0.9951× | 0.99× | ✅ |

Geometric mean 0.8581×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
