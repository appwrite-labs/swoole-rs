# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a457ec6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,944 | 15,214 | 0.9166× | 0.92× | ✅ |
| echo-1k | 17,053 | 18,165 | 0.9387× | 0.94× | ✅ |
| json | 22,123 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,573 | 17,777 | 0.9323× | 0.93× | ✅ |
| websocket | 6,558 | 8,188 | 0.8010× | 0.8× | ✅ |
| timer | 207,103 | 313,547 | 0.6605× | 0.65× | ✅ |
| app | 125,622 | 135,809 | 0.9250× | 0.93× | ✅ |
| http2 | 18,657 | 20,468 | 0.9115× | 0.91× | ✅ |
| table | 196,026 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 138,045 | 155,170 | 0.8896× | 0.89× | ✅ |
| task | 32,010 | 33,261 | 0.9624× | 0.96× | ✅ |
| mysql | 49,672 | 51,061 | 0.9728× | 0.97× | ✅ |
| http-client | 33,900 | 35,804 | 0.9468× | 0.94× | ✅ |
| client | 18,615 | 19,756 | 0.9422× | 0.94× | ✅ |
| socket | 19,745 | 21,072 | 0.9370× | 0.92× | ✅ |
| gzip | 194,180 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,048 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,712 | 4,762 | 0.9895× | 0.99× | ✅ |
| tcp-eof | 5,205 | 10,231 | 0.5087× | 0.51× | ✅ |
| udp | 7,395 | 7,986 | 0.9260× | 0.93× | ✅ |
| connect | 17,707 | 18,707 | 0.9465× | 0.94× | ✅ |
| https | 108,202 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,807 | 104,794 | 0.9906× | 0.99× | ✅ |
| atomic | 49,729 | 51,316 | 0.9691× | 0.97× | ✅ |
| process-mode | 15,049 | 16,960 | 0.8873× | 0.88× | ✅ |
| http2-concurrent | 360,131 | 408,881 | 0.8808× | 0.88× | ✅ |
| websocket-64k | 89,897 | 159,691 | 0.5629× | 0.57× | ✅ |
| websocket-fragmented | 12,020 | 16,327 | 0.7362× | 0.74× | ✅ |
| upload | 412,409 | 1,322,387 | 0.3119× | 0.32× | ✅ |
| static | 27,140 | 41,045 | 0.6612× | 0.66× | ✅ |
| pgsql | 23,393 | 24,595 | 0.9511× | 0.95× | ✅ |
| sqlite | 23,561 | 25,236 | 0.9336× | 0.93× | ✅ |
| curl | 85,806 | 90,351 | 0.9497× | 0.94× | ✅ |
| dns | 231,312 | 232,711 | 0.9940× | 0.99× | ✅ |

Geometric mean 0.8571×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
