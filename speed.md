# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,214 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,128 | 23,161 | 0.9554× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,951 | 313,538 | 0.6569× | 0.65× | ✅ |
| app | 125,356 | 135,926 | 0.9222× | 0.93× | ✅ |
| http2 | 18,710 | 20,469 | 0.9141× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,506 | 33,265 | 0.9772× | 0.96× | ✅ |
| mysql | 49,581 | 51,062 | 0.9710× | 0.97× | ✅ |
| http-client | 33,815 | 35,803 | 0.9445× | 0.94× | ✅ |
| client | 18,553 | 19,758 | 0.9390× | 0.94× | ✅ |
| socket | 19,677 | 21,072 | 0.9338× | 0.92× | ✅ |
| gzip | 194,170 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9444× | 0.93× | ✅ |
| connect | 17,744 | 18,703 | 0.9487× | 0.94× | ✅ |
| https | 108,216 | 108,803 | 0.9946× | 1.0× | ✅ |
| wss | 103,851 | 104,797 | 0.9910× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,095 | 16,963 | 0.8899× | 0.88× | ✅ |
| http2-concurrent | 361,868 | 409,069 | 0.8846× | 0.88× | ✅ |
| websocket-64k | 89,956 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,870 | 16,327 | 0.7270× | 0.73× | ✅ |
| upload | 413,444 | 1,322,390 | 0.3126× | 0.32× | ✅ |
| static | 27,325 | 41,080 | 0.6652× | 0.66× | ✅ |
| pgsql | 23,374 | 24,595 | 0.9503× | 0.95× | ✅ |
| sqlite | 23,537 | 25,218 | 0.9333× | 0.93× | ✅ |
| curl | 86,124 | 90,350 | 0.9532× | 0.94× | ✅ |
| dns | 231,725 | 233,110 | 0.9941× | 0.99× | ✅ |

Geometric mean 0.8579×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
