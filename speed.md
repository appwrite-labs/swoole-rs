# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `edab67b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,943 | 15,214 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,045 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,026 | 23,158 | 0.9511× | 0.95× | ✅ |
| service-health | 16,528 | 17,773 | 0.9299× | 0.93× | ✅ |
| websocket | 6,584 | 8,191 | 0.8038× | 0.81× | ✅ |
| timer | 203,444 | 313,764 | 0.6484× | 0.65× | ✅ |
| app | 125,339 | 135,899 | 0.9223× | 0.93× | ✅ |
| http2 | 18,649 | 20,464 | 0.9113× | 0.92× | ✅ |
| table | 195,481 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,422 | 154,651 | 0.8886× | 0.9× | ✅ |
| task | 31,990 | 33,265 | 0.9617× | 0.96× | ✅ |
| mysql | 49,495 | 51,064 | 0.9693× | 0.97× | ✅ |
| http-client | 33,647 | 35,802 | 0.9398× | 0.94× | ✅ |
| client | 18,439 | 19,757 | 0.9333× | 0.94× | ✅ |
| socket | 19,566 | 21,067 | 0.9288× | 0.92× | ✅ |
| gzip | 194,148 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,387 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.01× | ➖ |
| tcp-eof | 5,307 | 10,231 | 0.5187× | 0.52× | ✅ |
| udp | 7,576 | 7,987 | 0.9485× | 0.95× | ✅ |
| connect | 18,378 | 18,707 | 0.9824× | 0.98× | ✅ |
| https | 108,788 | 108,810 | 0.9998× | 1.0× | ✅ |
| wss | 104,399 | 104,801 | 0.9962× | 1.0× | ✅ |
| atomic | 49,688 | 51,312 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,687 | 16,963 | 0.9838× | 0.99× | ✅ |
| http2-concurrent | 359,853 | 408,877 | 0.8801× | 0.88× | ✅ |
| websocket-64k | 156,034 | 159,699 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,331 | 0.7847× | 0.79× | ✅ |
| upload | 478,085 | 1,322,376 | 0.3615× | 0.37× | ✅ |
| static | 29,347 | 41,074 | 0.7145× | 0.72× | ✅ |
| pgsql | 23,213 | 24,594 | 0.9439× | 0.95× | ✅ |
| sqlite | 23,483 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,863 | 90,407 | 0.9387× | 0.94× | ✅ |
| dns | 230,727 | 232,840 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8819×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
