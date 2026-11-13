# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3849fd4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,963 | 15,214 | 0.9178× | 0.92× | ✅ |
| echo-1k | 17,072 | 18,165 | 0.9398× | 0.94× | ✅ |
| json | 22,046 | 23,158 | 0.9520× | 0.95× | ✅ |
| service-health | 16,544 | 17,773 | 0.9308× | 0.93× | ✅ |
| websocket | 6,577 | 8,191 | 0.8029× | 0.81× | ✅ |
| timer | 203,476 | 313,533 | 0.6490× | 0.65× | ✅ |
| app | 125,404 | 135,956 | 0.9224× | 0.93× | ✅ |
| http2 | 18,652 | 20,464 | 0.9115× | 0.92× | ✅ |
| table | 195,518 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,456 | 154,651 | 0.8888× | 0.89× | ✅ |
| task | 32,015 | 33,265 | 0.9624× | 0.96× | ✅ |
| mysql | 49,529 | 51,064 | 0.9699× | 0.97× | ✅ |
| http-client | 33,713 | 35,806 | 0.9416× | 0.94× | ✅ |
| client | 18,496 | 19,757 | 0.9362× | 0.94× | ✅ |
| socket | 19,623 | 21,067 | 0.9314× | 0.92× | ✅ |
| gzip | 194,179 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,383 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,765 | 1.0053× | 1.01× | ➖ |
| tcp-eof | 5,313 | 10,228 | 0.5194× | 0.52× | ✅ |
| udp | 7,576 | 7,987 | 0.9485× | 0.95× | ✅ |
| connect | 18,403 | 18,703 | 0.9840× | 0.98× | ✅ |
| https | 108,801 | 108,813 | 0.9999× | 1.0× | ✅ |
| wss | 104,381 | 104,797 | 0.9960× | 1.0× | ✅ |
| atomic | 49,725 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,849 | 17,028 | 0.9308× | 0.94× | ✅ |
| http2-concurrent | 359,260 | 408,881 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,796 | 16,327 | 0.7837× | 0.79× | ✅ |
| upload | 478,133 | 1,322,380 | 0.3616× | 0.37× | ✅ |
| static | 29,144 | 41,045 | 0.7100× | 0.72× | ✅ |
| pgsql | 23,247 | 24,595 | 0.9452× | 0.95× | ✅ |
| sqlite | 23,518 | 25,236 | 0.9319× | 0.94× | ✅ |
| curl | 85,459 | 90,347 | 0.9459× | 0.94× | ✅ |
| dns | 230,761 | 232,713 | 0.9916× | 0.99× | ✅ |

Geometric mean 0.8809×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
