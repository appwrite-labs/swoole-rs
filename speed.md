# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6cdc1c3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,955 | 15,210 | 0.9175× | 0.92× | ✅ |
| echo-1k | 17,058 | 18,165 | 0.9391× | 0.94× | ✅ |
| json | 22,095 | 23,161 | 0.9540× | 0.95× | ✅ |
| service-health | 16,533 | 17,773 | 0.9302× | 0.93× | ✅ |
| websocket | 6,618 | 8,191 | 0.8079× | 0.8× | ✅ |
| timer | 205,869 | 313,611 | 0.6564× | 0.65× | ✅ |
| app | 125,310 | 135,967 | 0.9216× | 0.93× | ✅ |
| http2 | 18,736 | 20,464 | 0.9155× | 0.91× | ✅ |
| table | 195,617 | 201,865 | 0.9690× | 0.97× | ✅ |
| channel | 136,413 | 155,170 | 0.8791× | 0.88× | ✅ |
| task | 32,519 | 33,264 | 0.9776× | 0.96× | ✅ |
| mysql | 49,569 | 51,061 | 0.9708× | 0.97× | ✅ |
| http-client | 33,805 | 35,801 | 0.9443× | 0.94× | ✅ |
| client | 18,525 | 19,756 | 0.9377× | 0.94× | ✅ |
| socket | 19,649 | 21,072 | 0.9325× | 0.92× | ✅ |
| gzip | 194,157 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,057 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,707 | 4,762 | 0.9885× | 0.99× | ✅ |
| tcp-eof | 5,202 | 10,231 | 0.5084× | 0.51× | ✅ |
| udp | 7,512 | 7,987 | 0.9405× | 0.93× | ✅ |
| connect | 17,716 | 18,707 | 0.9470× | 0.94× | ✅ |
| https | 108,210 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,827 | 104,794 | 0.9908× | 0.99× | ✅ |
| atomic | 49,722 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,095 | 16,963 | 0.8899× | 0.88× | ✅ |
| http2-concurrent | 362,649 | 408,877 | 0.8869× | 0.88× | ✅ |
| websocket-64k | 89,951 | 159,694 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,855 | 16,331 | 0.7259× | 0.73× | ✅ |
| upload | 413,522 | 1,322,387 | 0.3127× | 0.32× | ✅ |
| static | 27,262 | 40,981 | 0.6652× | 0.66× | ✅ |
| pgsql | 23,370 | 24,594 | 0.9502× | 0.95× | ✅ |
| sqlite | 23,552 | 25,221 | 0.9338× | 0.93× | ✅ |
| curl | 85,936 | 90,310 | 0.9516× | 0.94× | ✅ |
| dns | 231,393 | 232,795 | 0.9940× | 0.99× | ✅ |

Geometric mean 0.8574×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
