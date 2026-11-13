# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `785a901` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,943 | 15,214 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,022 | 23,161 | 0.9508× | 0.95× | ✅ |
| service-health | 16,524 | 17,777 | 0.9295× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.81× | ✅ |
| timer | 203,453 | 313,589 | 0.6488× | 0.65× | ✅ |
| app | 125,350 | 135,818 | 0.9229× | 0.93× | ✅ |
| http2 | 18,645 | 20,468 | 0.9109× | 0.92× | ✅ |
| table | 195,477 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,419 | 154,654 | 0.8886× | 0.9× | ✅ |
| task | 31,993 | 33,265 | 0.9618× | 0.96× | ✅ |
| mysql | 49,465 | 51,064 | 0.9687× | 0.97× | ✅ |
| http-client | 33,641 | 35,809 | 0.9395× | 0.94× | ✅ |
| client | 18,435 | 19,756 | 0.9331× | 0.94× | ✅ |
| socket | 19,566 | 21,067 | 0.9288× | 0.92× | ✅ |
| gzip | 194,148 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,387 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,792 | 4,763 | 1.0060× | 1.01× | ➖ |
| tcp-eof | 5,307 | 10,231 | 0.5187× | 0.53× | ✅ |
| udp | 7,576 | 7,987 | 0.9485× | 0.95× | ✅ |
| connect | 18,378 | 18,707 | 0.9824× | 0.98× | ✅ |
| https | 108,788 | 108,810 | 0.9998× | 1.0× | ✅ |
| wss | 104,405 | 104,794 | 0.9963× | 1.0× | ✅ |
| atomic | 49,688 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,687 | 17,022 | 0.9803× | 0.99× | ✅ |
| http2-concurrent | 360,001 | 408,877 | 0.8805× | 0.88× | ✅ |
| websocket-64k | 156,028 | 159,695 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,327 | 0.7849× | 0.79× | ✅ |
| upload | 478,081 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,347 | 41,074 | 0.7145× | 0.72× | ✅ |
| pgsql | 23,207 | 24,598 | 0.9434× | 0.95× | ✅ |
| sqlite | 23,483 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,863 | 90,478 | 0.9379× | 0.94× | ✅ |
| dns | 230,746 | 232,833 | 0.9910× | 0.99× | ✅ |

Geometric mean 0.8817×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
