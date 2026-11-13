# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fc7896a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,823 | 15,213 | 0.9086× | 0.92× | ✅ |
| echo-1k | 16,928 | 18,165 | 0.9319× | 0.94× | ✅ |
| json | 21,926 | 23,161 | 0.9466× | 0.95× | ✅ |
| service-health | 16,423 | 17,773 | 0.9240× | 0.93× | ✅ |
| websocket | 6,470 | 8,188 | 0.7902× | 0.8× | ✅ |
| timer | 203,200 | 313,854 | 0.6474× | 0.65× | ✅ |
| app | 125,266 | 135,793 | 0.9225× | 0.93× | ✅ |
| http2 | 18,534 | 20,464 | 0.9057× | 0.91× | ✅ |
| table | 195,696 | 201,865 | 0.9694× | 0.97× | ✅ |
| channel | 137,383 | 155,170 | 0.8854× | 0.89× | ✅ |
| task | 31,883 | 33,265 | 0.9585× | 0.96× | ✅ |
| mysql | 49,430 | 51,021 | 0.9688× | 0.97× | ✅ |
| http-client | 33,665 | 35,796 | 0.9405× | 0.94× | ✅ |
| client | 18,424 | 19,757 | 0.9326× | 0.94× | ✅ |
| socket | 19,552 | 21,072 | 0.9279× | 0.92× | ✅ |
| gzip | 194,030 | 197,083 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 179,294 | 181,572 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,685 | 4,765 | 0.9831× | 0.99× | ✅ |
| tcp-eof | 5,208 | 10,231 | 0.5090× | 0.51× | ✅ |
| udp | 7,381 | 7,987 | 0.9241× | 0.93× | ✅ |
| connect | 17,508 | 18,707 | 0.9359× | 0.94× | ✅ |
| https | 108,033 | 108,813 | 0.9928× | 1.0× | ✅ |
| wss | 103,659 | 104,797 | 0.9891× | 0.99× | ✅ |
| atomic | 49,579 | 51,316 | 0.9662× | 0.97× | ✅ |
| process-mode | 14,892 | 16,963 | 0.8779× | 0.88× | ✅ |
| http2-concurrent | 357,925 | 409,069 | 0.8750× | 0.88× | ✅ |
| websocket-64k | 155,918 | 159,697 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,699 | 16,331 | 0.7776× | 0.78× | ✅ |
| upload | 478,017 | 1,322,387 | 0.3615× | 0.37× | ✅ |
| static | 27,236 | 41,016 | 0.6640× | 0.67× | ✅ |
| pgsql | 23,197 | 24,595 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,409 | 25,236 | 0.9276× | 0.93× | ✅ |
| curl | 85,752 | 90,474 | 0.9478× | 0.94× | ✅ |
| dns | 230,915 | 232,849 | 0.9917× | 0.99× | ✅ |

Geometric mean 0.8718×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
