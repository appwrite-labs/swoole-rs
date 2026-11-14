# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c61e611` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,922 | 15,210 | 0.9153× | 0.92× | ✅ |
| echo-1k | 17,037 | 18,165 | 0.9379× | 0.94× | ✅ |
| json | 22,120 | 23,161 | 0.9550× | 0.95× | ✅ |
| service-health | 16,556 | 17,773 | 0.9315× | 0.93× | ✅ |
| websocket | 6,553 | 8,191 | 0.8000× | 0.8× | ✅ |
| timer | 205,809 | 313,516 | 0.6565× | 0.65× | ✅ |
| app | 125,642 | 135,935 | 0.9243× | 0.93× | ✅ |
| http2 | 18,675 | 20,464 | 0.9126× | 0.91× | ✅ |
| table | 195,561 | 201,869 | 0.9688× | 0.97× | ✅ |
| channel | 137,228 | 155,171 | 0.8844× | 0.89× | ✅ |
| task | 32,469 | 33,264 | 0.9761× | 0.96× | ✅ |
| mysql | 49,684 | 51,061 | 0.9730× | 0.97× | ✅ |
| http-client | 33,928 | 35,799 | 0.9477× | 0.94× | ✅ |
| client | 18,601 | 19,756 | 0.9415× | 0.94× | ✅ |
| socket | 19,722 | 21,073 | 0.9359× | 0.92× | ✅ |
| gzip | 194,149 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,020 | 181,572 | 0.9859× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9905× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,511 | 7,986 | 0.9405× | 0.93× | ✅ |
| connect | 17,671 | 18,707 | 0.9446× | 0.94× | ✅ |
| https | 108,152 | 108,810 | 0.9940× | 1.0× | ✅ |
| wss | 103,770 | 104,801 | 0.9902× | 0.99× | ✅ |
| atomic | 49,705 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,076 | 16,956 | 0.8891× | 0.88× | ✅ |
| http2-concurrent | 361,009 | 409,768 | 0.8810× | 0.88× | ✅ |
| websocket-64k | 89,878 | 159,696 | 0.5628× | 0.57× | ✅ |
| websocket-fragmented | 11,998 | 16,331 | 0.7347× | 0.74× | ✅ |
| upload | 412,188 | 1,322,387 | 0.3117× | 0.32× | ✅ |
| static | 27,328 | 41,051 | 0.6657× | 0.66× | ✅ |
| pgsql | 23,344 | 24,595 | 0.9492× | 0.95× | ✅ |
| sqlite | 23,548 | 25,236 | 0.9331× | 0.93× | ✅ |
| curl | 85,908 | 90,477 | 0.9495× | 0.94× | ✅ |
| dns | 231,113 | 232,698 | 0.9932× | 0.99× | ✅ |

Geometric mean 0.8574×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
