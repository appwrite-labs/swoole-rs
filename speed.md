# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d968b88` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,949 | 15,213 | 0.9169× | 0.92× | ✅ |
| echo-1k | 17,055 | 18,165 | 0.9389× | 0.94× | ✅ |
| json | 22,035 | 23,161 | 0.9514× | 0.95× | ✅ |
| service-health | 16,531 | 17,773 | 0.9301× | 0.93× | ✅ |
| websocket | 6,555 | 8,191 | 0.8002× | 0.81× | ✅ |
| timer | 203,093 | 313,610 | 0.6476× | 0.65× | ✅ |
| app | 125,325 | 135,887 | 0.9223× | 0.93× | ✅ |
| http2 | 18,603 | 20,464 | 0.9091× | 0.92× | ✅ |
| table | 195,505 | 201,865 | 0.9685× | 0.97× | ✅ |
| channel | 137,447 | 154,654 | 0.8887× | 0.89× | ✅ |
| task | 31,999 | 33,264 | 0.9620× | 0.96× | ✅ |
| mysql | 49,546 | 50,915 | 0.9731× | 0.97× | ✅ |
| http-client | 33,699 | 35,807 | 0.9411× | 0.94× | ✅ |
| client | 18,482 | 19,757 | 0.9355× | 0.94× | ✅ |
| socket | 19,610 | 21,067 | 0.9308× | 0.92× | ✅ |
| gzip | 194,145 | 197,083 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,340 | 181,572 | 0.9877× | 0.99× | ✅ |
| tcp-length | 4,769 | 4,762 | 1.0015× | 1.01× | ➖ |
| tcp-eof | 5,292 | 10,228 | 0.5174× | 0.52× | ✅ |
| udp | 7,559 | 7,987 | 0.9464× | 0.95× | ✅ |
| connect | 17,900 | 18,707 | 0.9569× | 0.98× | ✅ |
| https | 108,635 | 108,810 | 0.9984× | 1.0× | ✅ |
| wss | 104,220 | 104,794 | 0.9945× | 1.0× | ✅ |
| atomic | 49,712 | 51,312 | 0.9688× | 0.97× | ✅ |
| process-mode | 15,476 | 16,957 | 0.9127× | 0.94× | ✅ |
| http2-concurrent | 357,468 | 409,415 | 0.8731× | 0.88× | ✅ |
| websocket-64k | 156,002 | 159,696 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,784 | 16,331 | 0.7828× | 0.79× | ✅ |
| upload | 477,885 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 28,516 | 41,074 | 0.6942× | 0.72× | ✅ |
| pgsql | 23,246 | 24,594 | 0.9452× | 0.95× | ✅ |
| sqlite | 23,510 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 85,747 | 90,466 | 0.9478× | 0.94× | ✅ |
| dns | 230,969 | 232,676 | 0.9927× | 0.99× | ✅ |

Geometric mean 0.8784×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
