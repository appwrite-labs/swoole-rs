# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7862691` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,998 | 15,214 | 0.9201× | 0.92× | ✅ |
| echo-1k | 17,108 | 18,165 | 0.9418× | 0.94× | ✅ |
| json | 22,099 | 23,161 | 0.9541× | 0.95× | ✅ |
| service-health | 16,598 | 17,777 | 0.9337× | 0.93× | ✅ |
| websocket | 6,730 | 8,188 | 0.8219× | 0.82× | ✅ |
| timer | 203,092 | 313,624 | 0.6476× | 0.65× | ✅ |
| app | 125,922 | 135,811 | 0.9272× | 0.93× | ✅ |
| http2 | 18,932 | 20,468 | 0.9250× | 0.94× | ✅ |
| table | 195,805 | 201,865 | 0.9700× | 0.97× | ✅ |
| channel | 138,434 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,006 | 33,268 | 0.9621× | 0.96× | ✅ |
| mysql | 49,644 | 51,061 | 0.9722× | 0.97× | ✅ |
| http-client | 33,702 | 35,808 | 0.9412× | 0.94× | ✅ |
| client | 18,548 | 19,757 | 0.9388× | 0.94× | ✅ |
| socket | 19,664 | 21,067 | 0.9334× | 0.92× | ✅ |
| gzip | 196,075 | 197,083 | 0.9949× | 1.0× | ✅ |
| websocket-deflate | 181,490 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 4,998 | 4,762 | 1.0495× | 1.05× | ➖ |
| tcp-eof | 5,533 | 10,228 | 0.5409× | 0.54× | ✅ |
| udp | 7,545 | 7,987 | 0.9447× | 0.95× | ✅ |
| connect | 18,314 | 18,707 | 0.9790× | 0.98× | ✅ |
| https | 108,851 | 108,810 | 1.0004× | 1.0× | ❌ |
| wss | 104,616 | 104,801 | 0.9982× | 1.0× | ✅ |
| atomic | 49,756 | 51,316 | 0.9696× | 0.97× | ✅ |
| process-mode | 16,555 | 17,024 | 0.9724× | 0.97× | ✅ |
| http2-concurrent | 359,422 | 408,881 | 0.8790× | 0.89× | ✅ |
| websocket-64k | 156,181 | 159,696 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,119 | 16,327 | 0.8035× | 0.79× | ✅ |
| upload | 478,523 | 1,322,380 | 0.3619× | 0.42× | ✅ |
| static | 29,354 | 41,016 | 0.7157× | 0.84× | ✅ |
| pgsql | 23,287 | 24,595 | 0.9468× | 0.95× | ✅ |
| sqlite | 23,551 | 25,232 | 0.9334× | 0.94× | ✅ |
| curl | 85,065 | 90,460 | 0.9404× | 0.95× | ✅ |
| dns | 230,297 | 233,230 | 0.9874× | 0.99× | ✅ |

Geometric mean 0.8871×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
