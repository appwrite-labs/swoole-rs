# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `134cbf5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,961 | 15,213 | 0.9177× | 0.92× | ✅ |
| echo-1k | 17,066 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,061 | 23,158 | 0.9526× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,584 | 8,191 | 0.8038× | 0.82× | ✅ |
| timer | 203,434 | 313,564 | 0.6488× | 0.65× | ✅ |
| app | 125,789 | 136,020 | 0.9248× | 0.93× | ✅ |
| http2 | 18,654 | 20,464 | 0.9115× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,439 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,139 | 33,265 | 0.9662× | 0.96× | ✅ |
| mysql | 49,482 | 51,060 | 0.9691× | 0.97× | ✅ |
| http-client | 33,745 | 35,809 | 0.9424× | 0.94× | ✅ |
| client | 18,477 | 19,756 | 0.9352× | 0.94× | ✅ |
| socket | 19,611 | 21,067 | 0.9309× | 0.92× | ✅ |
| gzip | 194,516 | 197,079 | 0.9870× | 1.0× | ✅ |
| websocket-deflate | 179,402 | 181,568 | 0.9881× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0056× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,374 | 18,703 | 0.9824× | 0.98× | ✅ |
| https | 108,799 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,481 | 104,801 | 0.9969× | 1.0× | ✅ |
| atomic | 49,678 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,679 | 17,028 | 0.9795× | 0.99× | ✅ |
| http2-concurrent | 358,472 | 408,881 | 0.8767× | 0.88× | ✅ |
| websocket-64k | 156,023 | 159,692 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,825 | 16,328 | 0.7855× | 0.79× | ✅ |
| upload | 477,978 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,256 | 41,045 | 0.7128× | 0.72× | ✅ |
| pgsql | 23,186 | 24,595 | 0.9427× | 0.95× | ✅ |
| sqlite | 23,466 | 25,232 | 0.9300× | 0.94× | ✅ |
| curl | 84,736 | 90,482 | 0.9365× | 0.94× | ✅ |
| dns | 230,675 | 233,263 | 0.9889× | 0.99× | ✅ |

Geometric mean 0.8824×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
