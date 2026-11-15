# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a671418` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,214 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,124 | 23,162 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 205,995 | 313,787 | 0.6565× | 0.65× | ✅ |
| app | 125,344 | 135,862 | 0.9226× | 0.93× | ✅ |
| http2 | 18,700 | 20,464 | 0.9138× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.89× | ✅ |
| task | 32,509 | 33,264 | 0.9773× | 0.96× | ✅ |
| mysql | 49,577 | 51,061 | 0.9709× | 0.97× | ✅ |
| http-client | 33,818 | 35,800 | 0.9446× | 0.94× | ✅ |
| client | 18,557 | 19,757 | 0.9393× | 0.94× | ✅ |
| socket | 19,674 | 21,072 | 0.9336× | 0.92× | ✅ |
| gzip | 194,170 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,094 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9444× | 0.93× | ✅ |
| connect | 17,745 | 18,703 | 0.9487× | 0.94× | ✅ |
| https | 108,202 | 108,813 | 0.9944× | 1.0× | ✅ |
| wss | 103,845 | 104,803 | 0.9909× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,094 | 16,960 | 0.8900× | 0.88× | ✅ |
| http2-concurrent | 361,857 | 409,073 | 0.8846× | 0.88× | ✅ |
| websocket-64k | 89,959 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,874 | 16,327 | 0.7273× | 0.73× | ✅ |
| upload | 413,458 | 1,322,387 | 0.3127× | 0.32× | ✅ |
| static | 27,313 | 41,051 | 0.6653× | 0.66× | ✅ |
| pgsql | 23,371 | 24,594 | 0.9503× | 0.95× | ✅ |
| sqlite | 23,540 | 25,236 | 0.9328× | 0.93× | ✅ |
| curl | 86,057 | 90,351 | 0.9525× | 0.94× | ✅ |
| dns | 231,498 | 232,633 | 0.9951× | 0.99× | ✅ |

Geometric mean 0.8579×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
