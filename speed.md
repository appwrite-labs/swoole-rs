# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2d2986d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,004 | 15,214 | 0.9205× | 0.92× | ✅ |
| echo-1k | 17,121 | 18,165 | 0.9425× | 0.94× | ✅ |
| json | 22,112 | 23,161 | 0.9547× | 0.95× | ✅ |
| service-health | 16,611 | 17,777 | 0.9344× | 0.93× | ✅ |
| websocket | 6,729 | 8,188 | 0.8218× | 0.82× | ✅ |
| timer | 202,995 | 313,625 | 0.6473× | 0.65× | ✅ |
| app | 125,969 | 135,962 | 0.9265× | 0.93× | ✅ |
| http2 | 18,923 | 20,468 | 0.9245× | 0.94× | ✅ |
| table | 195,803 | 201,865 | 0.9700× | 0.97× | ✅ |
| channel | 138,439 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,016 | 33,268 | 0.9624× | 0.96× | ✅ |
| mysql | 49,657 | 51,064 | 0.9724× | 0.97× | ✅ |
| http-client | 33,714 | 35,805 | 0.9416× | 0.94× | ✅ |
| client | 18,557 | 19,757 | 0.9393× | 0.94× | ✅ |
| socket | 19,674 | 21,067 | 0.9338× | 0.92× | ✅ |
| gzip | 196,084 | 197,080 | 0.9949× | 1.0× | ✅ |
| websocket-deflate | 181,490 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 4,995 | 4,762 | 1.0489× | 1.05× | ➖ |
| tcp-eof | 5,533 | 10,228 | 0.5409× | 0.54× | ✅ |
| udp | 7,545 | 7,986 | 0.9447× | 0.95× | ✅ |
| connect | 18,323 | 18,707 | 0.9795× | 0.98× | ✅ |
| https | 108,853 | 108,810 | 1.0004× | 1.0× | ❌ |
| wss | 104,616 | 104,793 | 0.9983× | 1.0× | ✅ |
| atomic | 49,758 | 51,316 | 0.9696× | 0.97× | ✅ |
| process-mode | 16,567 | 16,963 | 0.9767× | 0.97× | ✅ |
| http2-concurrent | 359,391 | 408,882 | 0.8790× | 0.89× | ✅ |
| websocket-64k | 156,181 | 159,696 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,119 | 16,327 | 0.8035× | 0.79× | ✅ |
| upload | 478,535 | 1,322,380 | 0.3619× | 0.42× | ✅ |
| static | 29,383 | 41,045 | 0.7159× | 0.84× | ✅ |
| pgsql | 23,294 | 24,595 | 0.9471× | 0.95× | ✅ |
| sqlite | 23,560 | 25,232 | 0.9337× | 0.94× | ✅ |
| curl | 85,064 | 90,513 | 0.9398× | 0.95× | ✅ |
| dns | 230,510 | 232,975 | 0.9894× | 0.99× | ✅ |

Geometric mean 0.8874×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
