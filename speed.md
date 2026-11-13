# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `873b7a6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,944 | 15,214 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,058 | 18,165 | 0.9391× | 0.94× | ✅ |
| json | 22,047 | 23,161 | 0.9519× | 0.95× | ✅ |
| service-health | 16,546 | 17,777 | 0.9307× | 0.93× | ✅ |
| websocket | 6,581 | 8,188 | 0.8037× | 0.81× | ✅ |
| timer | 203,539 | 313,555 | 0.6491× | 0.65× | ✅ |
| app | 125,332 | 135,917 | 0.9221× | 0.93× | ✅ |
| http2 | 18,693 | 20,468 | 0.9133× | 0.92× | ✅ |
| table | 195,498 | 201,865 | 0.9685× | 0.97× | ✅ |
| channel | 137,436 | 154,654 | 0.8887× | 0.9× | ✅ |
| task | 32,003 | 33,268 | 0.9620× | 0.96× | ✅ |
| mysql | 49,522 | 51,059 | 0.9699× | 0.97× | ✅ |
| http-client | 33,661 | 35,805 | 0.9401× | 0.94× | ✅ |
| client | 18,456 | 19,757 | 0.9341× | 0.94× | ✅ |
| socket | 19,584 | 21,067 | 0.9296× | 0.92× | ✅ |
| gzip | 194,145 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,387 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.01× | ➖ |
| tcp-eof | 5,307 | 10,228 | 0.5188× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,399 | 18,707 | 0.9835× | 0.98× | ✅ |
| https | 108,791 | 108,813 | 0.9998× | 1.0× | ✅ |
| wss | 104,402 | 104,803 | 0.9962× | 1.0× | ✅ |
| atomic | 49,705 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,852 | 16,959 | 0.9347× | 0.99× | ✅ |
| http2-concurrent | 360,080 | 408,881 | 0.8806× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,327 | 0.7849× | 0.79× | ✅ |
| upload | 478,086 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,224 | 41,045 | 0.7120× | 0.72× | ✅ |
| pgsql | 23,206 | 24,595 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,476 | 25,221 | 0.9308× | 0.94× | ✅ |
| curl | 84,857 | 90,313 | 0.9396× | 0.94× | ✅ |
| dns | 231,200 | 232,969 | 0.9924× | 0.99× | ✅ |

Geometric mean 0.8808×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
