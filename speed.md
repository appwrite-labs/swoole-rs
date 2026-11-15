# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,210 | 0.9185× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,128 | 23,161 | 0.9554× | 0.95× | ✅ |
| service-health | 16,567 | 17,773 | 0.9321× | 0.93× | ✅ |
| websocket | 6,636 | 8,188 | 0.8105× | 0.8× | ✅ |
| timer | 206,042 | 313,566 | 0.6571× | 0.65× | ✅ |
| app | 125,343 | 135,772 | 0.9232× | 0.93× | ✅ |
| http2 | 18,705 | 20,465 | 0.9140× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,509 | 33,265 | 0.9773× | 0.96× | ✅ |
| mysql | 49,578 | 51,058 | 0.9710× | 0.97× | ✅ |
| http-client | 33,812 | 35,804 | 0.9444× | 0.94× | ✅ |
| client | 18,556 | 19,757 | 0.9392× | 0.94× | ✅ |
| socket | 19,677 | 21,072 | 0.9338× | 0.92× | ✅ |
| gzip | 194,170 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,987 | 0.9444× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,213 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,849 | 104,801 | 0.9909× | 0.99× | ✅ |
| atomic | 49,702 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,095 | 16,962 | 0.8899× | 0.88× | ✅ |
| http2-concurrent | 362,142 | 408,877 | 0.8857× | 0.88× | ✅ |
| websocket-64k | 89,960 | 159,691 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,331 | 0.7269× | 0.73× | ✅ |
| upload | 413,023 | 1,322,387 | 0.3123× | 0.32× | ✅ |
| static | 27,284 | 41,016 | 0.6652× | 0.66× | ✅ |
| pgsql | 23,373 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,124 | 90,479 | 0.9519× | 0.94× | ✅ |
| dns | 230,910 | 232,892 | 0.9915× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
