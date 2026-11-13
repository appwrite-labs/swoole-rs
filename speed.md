# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ce94219` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,869 | 15,210 | 0.9118× | 0.92× | ✅ |
| echo-1k | 16,969 | 18,165 | 0.9342× | 0.94× | ✅ |
| json | 21,966 | 23,161 | 0.9484× | 0.95× | ✅ |
| service-health | 16,468 | 17,777 | 0.9264× | 0.93× | ✅ |
| websocket | 6,476 | 8,191 | 0.7906× | 0.81× | ✅ |
| timer | 203,095 | 313,509 | 0.6478× | 0.65× | ✅ |
| app | 125,251 | 135,864 | 0.9219× | 0.93× | ✅ |
| http2 | 18,471 | 20,464 | 0.9026× | 0.92× | ✅ |
| table | 195,445 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,387 | 154,651 | 0.8884× | 0.89× | ✅ |
| task | 31,907 | 33,268 | 0.9591× | 0.96× | ✅ |
| mysql | 49,480 | 51,032 | 0.9696× | 0.97× | ✅ |
| http-client | 33,667 | 35,799 | 0.9405× | 0.94× | ✅ |
| client | 18,422 | 19,757 | 0.9324× | 0.94× | ✅ |
| socket | 19,550 | 21,067 | 0.9280× | 0.92× | ✅ |
| gzip | 194,064 | 197,083 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,265 | 181,572 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,762 | 0.9849× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,375 | 7,987 | 0.9234× | 0.95× | ✅ |
| connect | 17,526 | 18,707 | 0.9369× | 0.98× | ✅ |
| https | 108,037 | 108,810 | 0.9929× | 1.0× | ✅ |
| wss | 103,623 | 104,801 | 0.9888× | 1.0× | ✅ |
| atomic | 49,585 | 51,316 | 0.9663× | 0.97× | ✅ |
| process-mode | 15,038 | 16,962 | 0.8866× | 0.94× | ✅ |
| http2-concurrent | 357,150 | 408,880 | 0.8735× | 0.88× | ✅ |
| websocket-64k | 155,920 | 159,696 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,698 | 16,327 | 0.7777× | 0.79× | ✅ |
| upload | 477,829 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 27,294 | 41,010 | 0.6655× | 0.72× | ✅ |
| pgsql | 23,160 | 24,594 | 0.9417× | 0.95× | ✅ |
| sqlite | 23,429 | 25,236 | 0.9284× | 0.94× | ✅ |
| curl | 85,660 | 90,470 | 0.9468× | 0.94× | ✅ |
| dns | 231,034 | 232,752 | 0.9926× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
