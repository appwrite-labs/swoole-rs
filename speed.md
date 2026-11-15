# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,213 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,938 | 313,607 | 0.6567× | 0.65× | ✅ |
| app | 125,384 | 135,797 | 0.9233× | 0.93× | ✅ |
| http2 | 18,709 | 20,469 | 0.9140× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,510 | 33,264 | 0.9773× | 0.96× | ✅ |
| mysql | 49,581 | 51,061 | 0.9710× | 0.97× | ✅ |
| http-client | 33,793 | 35,798 | 0.9440× | 0.94× | ✅ |
| client | 18,539 | 19,757 | 0.9383× | 0.94× | ✅ |
| socket | 19,664 | 21,072 | 0.9332× | 0.92× | ✅ |
| gzip | 194,182 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,560 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9444× | 0.93× | ✅ |
| connect | 17,745 | 18,703 | 0.9487× | 0.94× | ✅ |
| https | 108,202 | 108,813 | 0.9944× | 1.0× | ✅ |
| wss | 103,851 | 104,800 | 0.9909× | 0.99× | ✅ |
| atomic | 49,724 | 51,312 | 0.9691× | 0.97× | ✅ |
| process-mode | 15,094 | 16,963 | 0.8898× | 0.88× | ✅ |
| http2-concurrent | 361,596 | 409,627 | 0.8827× | 0.88× | ✅ |
| websocket-64k | 89,961 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,874 | 16,327 | 0.7273× | 0.73× | ✅ |
| upload | 413,460 | 1,322,387 | 0.3127× | 0.32× | ✅ |
| static | 27,313 | 41,051 | 0.6653× | 0.66× | ✅ |
| pgsql | 23,365 | 24,595 | 0.9500× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,124 | 90,350 | 0.9532× | 0.94× | ✅ |
| dns | 231,702 | 233,259 | 0.9933× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
