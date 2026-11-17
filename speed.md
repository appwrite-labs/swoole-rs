# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4691546` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,750 | 15,213 | 0.9038× | 0.91× | ✅ |
| echo-1k | 16,723 | 18,165 | 0.9206× | 0.93× | ✅ |
| json | 22,018 | 23,161 | 0.9506× | 0.95× | ✅ |
| service-health | 16,322 | 17,777 | 0.9182× | 0.92× | ✅ |
| websocket | 6,595 | 8,191 | 0.8052× | 0.8× | ✅ |
| timer | 205,334 | 313,586 | 0.6548× | 0.65× | ✅ |
| app | 126,596 | 135,987 | 0.9309× | 0.93× | ✅ |
| http2 | 18,963 | 20,464 | 0.9266× | 0.92× | ✅ |
| table | 194,804 | 201,865 | 0.9650× | 0.97× | ✅ |
| channel | 137,292 | 155,170 | 0.8848× | 0.88× | ✅ |
| task | 31,993 | 33,265 | 0.9618× | 0.96× | ✅ |
| mysql | 49,961 | 51,035 | 0.9790× | 0.97× | ✅ |
| http-client | 33,590 | 35,801 | 0.9382× | 0.93× | ✅ |
| gzip | 194,198 | 197,083 | 0.9854× | 0.99× | ✅ |
| websocket-deflate | 179,048 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,565 | 4,762 | 0.9586× | 0.96× | ✅ |
| tcp-eof | 5,074 | 10,228 | 0.4961× | 0.5× | ✅ |
| udp | 7,488 | 7,987 | 0.9376× | 0.93× | ✅ |
| connect | 17,342 | 18,707 | 0.9270× | 0.93× | ✅ |
| https | 105,960 | 108,810 | 0.9738× | 0.98× | ✅ |
| wss | 101,041 | 104,794 | 0.9642× | 0.97× | ✅ |
| atomic | 49,858 | 51,316 | 0.9716× | 0.97× | ✅ |
| process-mode | 15,218 | 16,959 | 0.8974× | 0.89× | ✅ |
| http2-concurrent | 361,884 | 408,882 | 0.8851× | 0.88× | ✅ |
| websocket-64k | 89,866 | 159,696 | 0.5627× | 0.57× | ✅ |
| websocket-fragmented | 11,714 | 16,331 | 0.7173× | 0.72× | ✅ |
| upload | 414,458 | 1,322,387 | 0.3134× | 0.32× | ✅ |
| static | 26,628 | 40,987 | 0.6497× | 0.65× | ✅ |
| client | 18,510 | 19,757 | 0.9369× | 0.94× | ✅ |
| socket | 19,687 | 21,073 | 0.9342× | 0.92× | ✅ |
| pgsql | 22,740 | 24,594 | 0.9246× | 0.93× | ✅ |
| sqlite | 23,448 | 25,236 | 0.9292× | 0.93× | ✅ |
| curl | 86,482 | 90,381 | 0.9569× | 0.94× | ✅ |
| dns | 229,877 | 233,006 | 0.9866× | 0.99× | ✅ |

Geometric mean 0.8515×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
