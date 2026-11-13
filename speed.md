# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b8b65e3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,861 | 15,214 | 0.9111× | 0.92× | ✅ |
| echo-1k | 16,962 | 18,165 | 0.9338× | 0.94× | ✅ |
| json | 21,957 | 23,161 | 0.9480× | 0.95× | ✅ |
| service-health | 16,456 | 17,777 | 0.9257× | 0.93× | ✅ |
| websocket | 6,480 | 8,191 | 0.7911× | 0.8× | ✅ |
| timer | 203,061 | 313,566 | 0.6476× | 0.65× | ✅ |
| app | 125,331 | 135,940 | 0.9220× | 0.93× | ✅ |
| http2 | 18,487 | 20,464 | 0.9034× | 0.91× | ✅ |
| table | 195,430 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,371 | 155,170 | 0.8853× | 0.89× | ✅ |
| task | 31,918 | 33,268 | 0.9594× | 0.96× | ✅ |
| mysql | 49,476 | 51,055 | 0.9691× | 0.97× | ✅ |
| http-client | 33,631 | 35,806 | 0.9392× | 0.94× | ✅ |
| client | 18,421 | 19,753 | 0.9326× | 0.94× | ✅ |
| socket | 19,545 | 21,067 | 0.9278× | 0.92× | ✅ |
| gzip | 194,063 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,266 | 181,572 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,692 | 4,762 | 0.9853× | 0.99× | ✅ |
| tcp-eof | 5,215 | 10,231 | 0.5097× | 0.51× | ✅ |
| udp | 7,378 | 7,987 | 0.9238× | 0.93× | ✅ |
| connect | 17,541 | 18,707 | 0.9377× | 0.95× | ✅ |
| https | 108,030 | 108,813 | 0.9928× | 1.0× | ✅ |
| wss | 103,633 | 104,794 | 0.9889× | 0.99× | ✅ |
| atomic | 49,583 | 51,316 | 0.9662× | 0.97× | ✅ |
| process-mode | 14,902 | 16,962 | 0.8785× | 0.89× | ✅ |
| http2-concurrent | 357,328 | 409,442 | 0.8727× | 0.88× | ✅ |
| websocket-64k | 155,926 | 159,693 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,706 | 16,331 | 0.7781× | 0.78× | ✅ |
| upload | 477,820 | 1,322,382 | 0.3613× | 0.37× | ✅ |
| static | 27,297 | 41,010 | 0.6656× | 0.67× | ✅ |
| pgsql | 23,149 | 24,595 | 0.9412× | 0.95× | ✅ |
| sqlite | 23,413 | 25,236 | 0.9277× | 0.93× | ✅ |
| curl | 85,654 | 90,473 | 0.9467× | 0.94× | ✅ |
| dns | 231,212 | 233,361 | 0.9908× | 0.99× | ✅ |

Geometric mean 0.8720×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
