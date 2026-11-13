# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bb9459b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,869 | 15,214 | 0.9116× | 0.92× | ✅ |
| echo-1k | 16,970 | 18,165 | 0.9342× | 0.94× | ✅ |
| json | 21,961 | 23,161 | 0.9482× | 0.95× | ✅ |
| service-health | 16,453 | 17,777 | 0.9256× | 0.93× | ✅ |
| websocket | 6,476 | 8,188 | 0.7909× | 0.81× | ✅ |
| timer | 203,080 | 313,631 | 0.6475× | 0.65× | ✅ |
| app | 125,343 | 135,821 | 0.9229× | 0.93× | ✅ |
| http2 | 18,486 | 20,467 | 0.9032× | 0.92× | ✅ |
| table | 195,427 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,369 | 155,170 | 0.8853× | 0.89× | ✅ |
| task | 31,963 | 33,267 | 0.9608× | 0.96× | ✅ |
| mysql | 49,444 | 51,064 | 0.9683× | 0.97× | ✅ |
| http-client | 33,620 | 35,808 | 0.9389× | 0.94× | ✅ |
| client | 18,413 | 19,757 | 0.9320× | 0.94× | ✅ |
| socket | 19,541 | 21,067 | 0.9276× | 0.92× | ✅ |
| gzip | 194,064 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,261 | 181,572 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,765 | 0.9842× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,375 | 7,986 | 0.9235× | 0.95× | ✅ |
| connect | 17,618 | 18,707 | 0.9418× | 0.98× | ✅ |
| https | 108,026 | 108,810 | 0.9928× | 1.0× | ✅ |
| wss | 103,623 | 104,800 | 0.9888× | 1.0× | ✅ |
| atomic | 49,575 | 51,316 | 0.9661× | 0.97× | ✅ |
| process-mode | 15,038 | 16,960 | 0.8867× | 0.94× | ✅ |
| http2-concurrent | 357,593 | 408,877 | 0.8746× | 0.88× | ✅ |
| websocket-64k | 155,924 | 159,691 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,705 | 16,331 | 0.7779× | 0.79× | ✅ |
| upload | 477,824 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 27,277 | 40,981 | 0.6656× | 0.72× | ✅ |
| pgsql | 23,150 | 24,594 | 0.9413× | 0.95× | ✅ |
| sqlite | 23,413 | 25,236 | 0.9278× | 0.94× | ✅ |
| curl | 85,651 | 90,354 | 0.9480× | 0.94× | ✅ |
| dns | 230,813 | 233,104 | 0.9902× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
