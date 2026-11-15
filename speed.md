# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,213 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,937 | 313,746 | 0.6564× | 0.65× | ✅ |
| app | 125,351 | 135,937 | 0.9221× | 0.93× | ✅ |
| http2 | 18,710 | 20,469 | 0.9141× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,517 | 33,264 | 0.9775× | 0.96× | ✅ |
| mysql | 49,577 | 51,064 | 0.9709× | 0.97× | ✅ |
| http-client | 33,796 | 35,797 | 0.9441× | 0.94× | ✅ |
| client | 18,539 | 19,757 | 0.9383× | 0.94× | ✅ |
| socket | 19,665 | 21,073 | 0.9332× | 0.92× | ✅ |
| gzip | 194,182 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,560 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,987 | 0.9443× | 0.93× | ✅ |
| connect | 17,744 | 18,703 | 0.9487× | 0.94× | ✅ |
| https | 108,213 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,849 | 104,794 | 0.9910× | 0.99× | ✅ |
| atomic | 49,720 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 15,094 | 16,963 | 0.8898× | 0.88× | ✅ |
| http2-concurrent | 361,564 | 409,075 | 0.8839× | 0.88× | ✅ |
| websocket-64k | 89,958 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,331 | 0.7269× | 0.73× | ✅ |
| upload | 413,456 | 1,322,393 | 0.3127× | 0.32× | ✅ |
| static | 27,342 | 41,080 | 0.6656× | 0.66× | ✅ |
| pgsql | 23,385 | 24,595 | 0.9508× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,124 | 90,350 | 0.9532× | 0.94× | ✅ |
| dns | 230,637 | 232,994 | 0.9899× | 0.99× | ✅ |

Geometric mean 0.8577×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
