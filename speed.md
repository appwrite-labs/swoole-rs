# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0c0e15d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,948 | 15,214 | 0.9168× | 0.92× | ✅ |
| echo-1k | 17,054 | 18,162 | 0.9390× | 0.94× | ✅ |
| json | 22,059 | 23,158 | 0.9526× | 0.95× | ✅ |
| service-health | 16,561 | 17,777 | 0.9316× | 0.93× | ✅ |
| websocket | 6,562 | 8,188 | 0.8015× | 0.81× | ✅ |
| timer | 203,105 | 313,527 | 0.6478× | 0.65× | ✅ |
| app | 125,441 | 135,932 | 0.9228× | 0.93× | ✅ |
| http2 | 18,610 | 20,465 | 0.9093× | 0.92× | ✅ |
| table | 195,535 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,473 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,003 | 33,265 | 0.9621× | 0.96× | ✅ |
| mysql | 49,582 | 51,064 | 0.9710× | 0.97× | ✅ |
| http-client | 33,751 | 35,806 | 0.9426× | 0.94× | ✅ |
| client | 18,513 | 19,757 | 0.9370× | 0.94× | ✅ |
| socket | 19,640 | 21,067 | 0.9323× | 0.92× | ✅ |
| gzip | 194,149 | 197,083 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,347 | 181,568 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,773 | 4,762 | 1.0023× | 1.01× | ➖ |
| tcp-eof | 5,296 | 10,228 | 0.5178× | 0.52× | ✅ |
| udp | 7,554 | 7,987 | 0.9458× | 0.95× | ✅ |
| connect | 17,891 | 18,707 | 0.9564× | 0.98× | ✅ |
| https | 108,712 | 108,810 | 0.9991× | 1.0× | ✅ |
| wss | 104,294 | 104,794 | 0.9952× | 1.0× | ✅ |
| atomic | 49,742 | 51,312 | 0.9694× | 0.97× | ✅ |
| process-mode | 15,639 | 16,959 | 0.9222× | 0.94× | ✅ |
| http2-concurrent | 357,727 | 408,881 | 0.8749× | 0.88× | ✅ |
| websocket-64k | 156,001 | 159,696 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,788 | 16,328 | 0.7832× | 0.79× | ✅ |
| upload | 477,899 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 28,574 | 41,045 | 0.6962× | 0.72× | ✅ |
| pgsql | 23,232 | 24,594 | 0.9446× | 0.95× | ✅ |
| sqlite | 23,503 | 25,236 | 0.9313× | 0.94× | ✅ |
| curl | 85,738 | 90,350 | 0.9489× | 0.94× | ✅ |
| dns | 230,984 | 232,946 | 0.9916× | 0.99× | ✅ |

Geometric mean 0.8790×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
