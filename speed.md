# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e5458f5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,854 | 15,213 | 0.9107× | 0.92× | ✅ |
| echo-1k | 16,959 | 18,165 | 0.9336× | 0.94× | ✅ |
| json | 21,971 | 23,158 | 0.9487× | 0.95× | ✅ |
| service-health | 16,467 | 17,777 | 0.9263× | 0.93× | ✅ |
| websocket | 6,467 | 8,188 | 0.7898× | 0.81× | ✅ |
| timer | 203,016 | 313,529 | 0.6475× | 0.65× | ✅ |
| app | 125,339 | 135,942 | 0.9220× | 0.93× | ✅ |
| http2 | 18,452 | 20,464 | 0.9017× | 0.92× | ✅ |
| table | 195,441 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,379 | 154,651 | 0.8883× | 0.89× | ✅ |
| task | 31,899 | 33,261 | 0.9591× | 0.96× | ✅ |
| mysql | 49,482 | 51,060 | 0.9691× | 0.97× | ✅ |
| http-client | 33,663 | 35,808 | 0.9401× | 0.94× | ✅ |
| client | 18,418 | 19,757 | 0.9323× | 0.94× | ✅ |
| socket | 19,542 | 21,067 | 0.9276× | 0.92× | ✅ |
| gzip | 194,053 | 197,083 | 0.9846× | 0.99× | ✅ |
| websocket-deflate | 179,252 | 181,572 | 0.9872× | 0.99× | ✅ |
| tcp-length | 4,678 | 4,765 | 0.9816× | 1.01× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.52× | ✅ |
| udp | 7,531 | 7,986 | 0.9430× | 0.95× | ✅ |
| connect | 17,635 | 18,707 | 0.9427× | 0.98× | ✅ |
| https | 108,041 | 108,810 | 0.9929× | 1.0× | ✅ |
| wss | 103,607 | 104,794 | 0.9887× | 1.0× | ✅ |
| atomic | 49,584 | 51,316 | 0.9662× | 0.97× | ✅ |
| process-mode | 15,026 | 16,956 | 0.8862× | 0.94× | ✅ |
| http2-concurrent | 356,653 | 408,882 | 0.8723× | 0.88× | ✅ |
| websocket-64k | 155,913 | 159,697 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,696 | 16,331 | 0.7774× | 0.79× | ✅ |
| upload | 477,812 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 27,326 | 41,042 | 0.6658× | 0.72× | ✅ |
| pgsql | 23,143 | 24,594 | 0.9410× | 0.95× | ✅ |
| sqlite | 23,409 | 25,236 | 0.9276× | 0.94× | ✅ |
| curl | 85,637 | 90,464 | 0.9466× | 0.94× | ✅ |
| dns | 230,749 | 232,730 | 0.9915× | 0.99× | ✅ |

Geometric mean 0.8727×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
