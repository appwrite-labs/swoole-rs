# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e8c0fe6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,952 | 15,213 | 0.9171× | 0.92× | ✅ |
| echo-1k | 17,057 | 18,166 | 0.9390× | 0.94× | ✅ |
| json | 22,050 | 23,161 | 0.9520× | 0.95× | ✅ |
| service-health | 16,545 | 17,773 | 0.9309× | 0.93× | ✅ |
| websocket | 6,573 | 8,191 | 0.8025× | 0.82× | ✅ |
| timer | 203,277 | 313,497 | 0.6484× | 0.65× | ✅ |
| app | 125,882 | 135,923 | 0.9261× | 0.93× | ✅ |
| http2 | 18,637 | 20,464 | 0.9107× | 0.93× | ✅ |
| table | 195,736 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,887 | 154,654 | 0.8981× | 0.9× | ✅ |
| task | 32,201 | 33,268 | 0.9679× | 0.96× | ✅ |
| mysql | 49,628 | 51,064 | 0.9719× | 0.97× | ✅ |
| http-client | 33,676 | 35,805 | 0.9405× | 0.94× | ✅ |
| client | 18,485 | 19,757 | 0.9356× | 0.94× | ✅ |
| socket | 19,604 | 21,067 | 0.9306× | 0.92× | ✅ |
| gzip | 195,983 | 197,079 | 0.9944× | 1.0× | ✅ |
| websocket-deflate | 181,332 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,777 | 4,765 | 1.0024× | 1.05× | ➖ |
| tcp-eof | 5,325 | 10,231 | 0.5204× | 0.54× | ✅ |
| udp | 7,560 | 7,987 | 0.9465× | 0.95× | ✅ |
| connect | 18,386 | 18,707 | 0.9829× | 0.98× | ✅ |
| https | 108,791 | 108,803 | 0.9999× | 1.0× | ✅ |
| wss | 104,464 | 104,794 | 0.9969× | 1.0× | ✅ |
| atomic | 49,687 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,784 | 16,959 | 0.9897× | 0.99× | ✅ |
| http2-concurrent | 359,834 | 408,882 | 0.8800× | 0.88× | ✅ |
| websocket-64k | 156,025 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,327 | 0.7847× | 0.79× | ✅ |
| upload | 478,451 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,200 | 41,010 | 0.7120× | 0.72× | ✅ |
| pgsql | 23,218 | 24,595 | 0.9440× | 0.95× | ✅ |
| sqlite | 23,500 | 25,236 | 0.9312× | 0.94× | ✅ |
| curl | 84,611 | 90,321 | 0.9368× | 0.94× | ✅ |
| dns | 230,243 | 232,954 | 0.9884× | 0.99× | ✅ |

Geometric mean 0.8832×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
