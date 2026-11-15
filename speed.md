# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,214 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,688 | 313,615 | 0.6559× | 0.65× | ✅ |
| app | 125,329 | 135,957 | 0.9218× | 0.93× | ✅ |
| http2 | 18,669 | 20,467 | 0.9121× | 0.91× | ✅ |
| table | 196,026 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,506 | 33,264 | 0.9772× | 0.96× | ✅ |
| mysql | 49,580 | 51,052 | 0.9712× | 0.97× | ✅ |
| http-client | 33,792 | 35,803 | 0.9438× | 0.94× | ✅ |
| client | 18,534 | 19,757 | 0.9381× | 0.94× | ✅ |
| socket | 19,664 | 21,073 | 0.9332× | 0.92× | ✅ |
| gzip | 194,182 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,560 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9444× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,213 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,855 | 104,794 | 0.9910× | 0.99× | ✅ |
| atomic | 49,720 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 15,094 | 16,957 | 0.8902× | 0.88× | ✅ |
| http2-concurrent | 361,860 | 409,073 | 0.8846× | 0.88× | ✅ |
| websocket-64k | 89,954 | 159,691 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,327 | 0.7270× | 0.73× | ✅ |
| upload | 413,444 | 1,322,388 | 0.3126× | 0.32× | ✅ |
| static | 27,325 | 41,051 | 0.6656× | 0.66× | ✅ |
| pgsql | 23,375 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,540 | 25,236 | 0.9328× | 0.93× | ✅ |
| curl | 86,121 | 90,333 | 0.9534× | 0.94× | ✅ |
| dns | 231,539 | 233,044 | 0.9935× | 0.99× | ✅ |

Geometric mean 0.8577×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
