# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3a58c76` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,870 | 15,210 | 0.9119× | 0.92× | ✅ |
| echo-1k | 16,974 | 18,165 | 0.9344× | 0.94× | ✅ |
| json | 21,974 | 23,161 | 0.9488× | 0.95× | ✅ |
| service-health | 16,473 | 17,777 | 0.9267× | 0.93× | ✅ |
| websocket | 6,480 | 8,188 | 0.7914× | 0.81× | ✅ |
| timer | 203,092 | 313,713 | 0.6474× | 0.65× | ✅ |
| app | 125,360 | 135,933 | 0.9222× | 0.93× | ✅ |
| http2 | 18,474 | 20,464 | 0.9027× | 0.92× | ✅ |
| table | 195,447 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,385 | 154,651 | 0.8884× | 0.89× | ✅ |
| task | 31,908 | 33,265 | 0.9592× | 0.96× | ✅ |
| mysql | 49,477 | 51,061 | 0.9690× | 0.97× | ✅ |
| http-client | 33,638 | 35,811 | 0.9393× | 0.94× | ✅ |
| client | 18,425 | 19,756 | 0.9326× | 0.94× | ✅ |
| socket | 19,553 | 21,067 | 0.9281× | 0.92× | ✅ |
| gzip | 194,064 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,261 | 181,568 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,765 | 0.9843× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,375 | 7,986 | 0.9235× | 0.95× | ✅ |
| connect | 17,535 | 18,703 | 0.9376× | 0.98× | ✅ |
| https | 108,021 | 108,813 | 0.9927× | 1.0× | ✅ |
| wss | 103,624 | 104,794 | 0.9888× | 1.0× | ✅ |
| atomic | 49,587 | 51,316 | 0.9663× | 0.97× | ✅ |
| process-mode | 15,061 | 17,024 | 0.8847× | 0.94× | ✅ |
| http2-concurrent | 357,422 | 408,880 | 0.8741× | 0.88× | ✅ |
| websocket-64k | 155,926 | 159,694 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,705 | 16,327 | 0.7781× | 0.79× | ✅ |
| upload | 477,813 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 27,278 | 40,981 | 0.6656× | 0.72× | ✅ |
| pgsql | 23,153 | 24,595 | 0.9414× | 0.95× | ✅ |
| sqlite | 23,412 | 25,221 | 0.9283× | 0.94× | ✅ |
| curl | 85,655 | 90,351 | 0.9480× | 0.94× | ✅ |
| dns | 230,602 | 232,881 | 0.9902× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
