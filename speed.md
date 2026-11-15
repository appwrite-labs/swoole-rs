# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,214 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,973 | 313,536 | 0.6569× | 0.65× | ✅ |
| app | 125,349 | 135,911 | 0.9223× | 0.93× | ✅ |
| http2 | 18,698 | 20,468 | 0.9135× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,506 | 33,268 | 0.9771× | 0.96× | ✅ |
| mysql | 49,577 | 51,067 | 0.9708× | 0.97× | ✅ |
| http-client | 33,812 | 35,800 | 0.9445× | 0.94× | ✅ |
| client | 18,556 | 19,757 | 0.9392× | 0.94× | ✅ |
| socket | 19,677 | 21,073 | 0.9338× | 0.92× | ✅ |
| gzip | 194,170 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,987 | 0.9443× | 0.93× | ✅ |
| connect | 17,741 | 18,707 | 0.9484× | 0.94× | ✅ |
| https | 108,199 | 108,803 | 0.9944× | 1.0× | ✅ |
| wss | 103,848 | 104,801 | 0.9909× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,095 | 16,960 | 0.8900× | 0.88× | ✅ |
| http2-concurrent | 361,941 | 409,073 | 0.8848× | 0.88× | ✅ |
| websocket-64k | 89,954 | 159,699 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,874 | 16,328 | 0.7272× | 0.73× | ✅ |
| upload | 413,520 | 1,322,389 | 0.3127× | 0.32× | ✅ |
| static | 27,267 | 40,993 | 0.6652× | 0.66× | ✅ |
| pgsql | 23,374 | 24,595 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,540 | 25,232 | 0.9329× | 0.93× | ✅ |
| curl | 86,120 | 90,351 | 0.9532× | 0.94× | ✅ |
| dns | 231,693 | 232,926 | 0.9947× | 0.99× | ✅ |

Geometric mean 0.8579×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
