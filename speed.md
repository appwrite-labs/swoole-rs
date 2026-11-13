# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5cedc9b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,873 | 15,214 | 0.9119× | 0.92× | ✅ |
| echo-1k | 16,970 | 18,165 | 0.9342× | 0.94× | ✅ |
| json | 21,971 | 23,161 | 0.9486× | 0.95× | ✅ |
| service-health | 16,470 | 17,777 | 0.9265× | 0.93× | ✅ |
| websocket | 6,475 | 8,188 | 0.7909× | 0.81× | ✅ |
| timer | 203,074 | 313,604 | 0.6475× | 0.65× | ✅ |
| app | 125,366 | 135,945 | 0.9222× | 0.93× | ✅ |
| http2 | 18,467 | 20,468 | 0.9022× | 0.92× | ✅ |
| table | 195,444 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,389 | 154,654 | 0.8884× | 0.89× | ✅ |
| task | 31,908 | 33,268 | 0.9591× | 0.96× | ✅ |
| mysql | 49,428 | 51,061 | 0.9680× | 0.97× | ✅ |
| http-client | 33,636 | 35,809 | 0.9393× | 0.94× | ✅ |
| client | 18,424 | 19,757 | 0.9326× | 0.94× | ✅ |
| socket | 19,552 | 21,067 | 0.9281× | 0.92× | ✅ |
| gzip | 194,064 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,261 | 181,568 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,762 | 0.9849× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,228 | 0.5097× | 0.52× | ✅ |
| udp | 7,375 | 7,987 | 0.9234× | 0.95× | ✅ |
| connect | 17,535 | 18,707 | 0.9374× | 0.98× | ✅ |
| https | 108,018 | 108,813 | 0.9927× | 1.0× | ✅ |
| wss | 103,624 | 104,794 | 0.9888× | 1.0× | ✅ |
| atomic | 49,590 | 51,316 | 0.9664× | 0.97× | ✅ |
| process-mode | 15,064 | 16,956 | 0.8884× | 0.94× | ✅ |
| http2-concurrent | 357,357 | 408,881 | 0.8740× | 0.88× | ✅ |
| websocket-64k | 155,920 | 159,696 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,708 | 16,327 | 0.7783× | 0.79× | ✅ |
| upload | 477,814 | 1,322,382 | 0.3613× | 0.37× | ✅ |
| static | 27,365 | 41,068 | 0.6663× | 0.72× | ✅ |
| pgsql | 23,153 | 24,594 | 0.9414× | 0.95× | ✅ |
| sqlite | 23,416 | 25,236 | 0.9279× | 0.94× | ✅ |
| curl | 85,648 | 90,321 | 0.9483× | 0.94× | ✅ |
| dns | 230,617 | 232,744 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8725×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
