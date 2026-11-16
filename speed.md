# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `94109d3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,864 | 15,210 | 0.9115× | 0.91× | ✅ |
| echo-1k | 16,898 | 18,166 | 0.9302× | 0.93× | ✅ |
| json | 22,016 | 23,161 | 0.9505× | 0.95× | ✅ |
| service-health | 16,450 | 17,777 | 0.9254× | 0.93× | ✅ |
| websocket | 6,551 | 8,191 | 0.7998× | 0.8× | ✅ |
| timer | 205,844 | 313,562 | 0.6565× | 0.65× | ✅ |
| app | 125,649 | 135,913 | 0.9245× | 0.93× | ✅ |
| http2 | 18,891 | 20,464 | 0.9231× | 0.91× | ✅ |
| table | 195,112 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 136,817 | 155,170 | 0.8817× | 0.88× | ✅ |
| task | 32,423 | 33,261 | 0.9748× | 0.96× | ✅ |
| mysql | 49,630 | 51,061 | 0.9720× | 0.97× | ✅ |
| http-client | 33,534 | 35,798 | 0.9368× | 0.93× | ✅ |
| gzip | 194,014 | 197,079 | 0.9844× | 0.99× | ✅ |
| websocket-deflate | 178,821 | 181,560 | 0.9849× | 0.99× | ✅ |
| tcp-length | 4,654 | 4,762 | 0.9773× | 0.98× | ✅ |
| tcp-eof | 5,144 | 10,231 | 0.5028× | 0.51× | ✅ |
| udp | 7,518 | 7,987 | 0.9413× | 0.93× | ✅ |
| connect | 17,639 | 18,707 | 0.9429× | 0.94× | ✅ |
| https | 108,112 | 108,810 | 0.9936× | 1.0× | ✅ |
| wss | 103,744 | 104,800 | 0.9899× | 0.99× | ✅ |
| atomic | 49,967 | 51,316 | 0.9737× | 0.97× | ✅ |
| process-mode | 15,070 | 16,956 | 0.8888× | 0.88× | ✅ |
| http2-concurrent | 357,834 | 409,072 | 0.8747× | 0.88× | ✅ |
| websocket-64k | 89,853 | 159,696 | 0.5626× | 0.57× | ✅ |
| websocket-fragmented | 11,980 | 16,327 | 0.7338× | 0.72× | ✅ |
| upload | 414,011 | 1,322,388 | 0.3131× | 0.32× | ✅ |
| static | 26,989 | 41,080 | 0.6570× | 0.66× | ✅ |
| client | 18,544 | 19,756 | 0.9386× | 0.94× | ✅ |
| socket | 19,701 | 21,069 | 0.9351× | 0.92× | ✅ |
| pgsql | 23,306 | 24,594 | 0.9476× | 0.95× | ✅ |
| sqlite | 23,530 | 25,236 | 0.9324× | 0.93× | ✅ |
| curl | 86,027 | 90,460 | 0.9510× | 0.94× | ✅ |
| dns | 230,342 | 232,916 | 0.9889× | 0.99× | ✅ |

Geometric mean 0.8553×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
