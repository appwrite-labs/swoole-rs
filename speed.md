# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `045f558` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,869 | 15,210 | 0.9118× | 0.92× | ✅ |
| echo-1k | 16,969 | 18,162 | 0.9344× | 0.94× | ✅ |
| json | 21,964 | 23,161 | 0.9483× | 0.95× | ✅ |
| service-health | 16,464 | 17,777 | 0.9262× | 0.93× | ✅ |
| websocket | 6,479 | 8,188 | 0.7913× | 0.81× | ✅ |
| timer | 203,305 | 314,312 | 0.6468× | 0.65× | ✅ |
| app | 125,320 | 135,990 | 0.9215× | 0.93× | ✅ |
| http2 | 18,475 | 20,464 | 0.9028× | 0.92× | ✅ |
| table | 195,438 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,376 | 154,654 | 0.8883× | 0.89× | ✅ |
| task | 31,934 | 33,268 | 0.9599× | 0.96× | ✅ |
| mysql | 49,461 | 51,061 | 0.9687× | 0.97× | ✅ |
| http-client | 33,625 | 35,800 | 0.9393× | 0.94× | ✅ |
| client | 18,406 | 19,756 | 0.9317× | 0.94× | ✅ |
| socket | 19,531 | 21,073 | 0.9268× | 0.92× | ✅ |
| gzip | 194,069 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,306 | 181,568 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,763 | 0.9848× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,375 | 7,986 | 0.9235× | 0.95× | ✅ |
| connect | 17,552 | 18,707 | 0.9383× | 0.98× | ✅ |
| https | 108,017 | 108,810 | 0.9927× | 1.0× | ✅ |
| wss | 103,624 | 104,797 | 0.9888× | 1.0× | ✅ |
| atomic | 49,581 | 51,316 | 0.9662× | 0.97× | ✅ |
| process-mode | 15,041 | 16,959 | 0.8869× | 0.94× | ✅ |
| http2-concurrent | 357,415 | 408,881 | 0.8741× | 0.88× | ✅ |
| websocket-64k | 155,916 | 159,699 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,708 | 16,327 | 0.7784× | 0.79× | ✅ |
| upload | 477,890 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 27,266 | 40,981 | 0.6653× | 0.72× | ✅ |
| pgsql | 23,136 | 24,594 | 0.9407× | 0.95× | ✅ |
| sqlite | 23,407 | 25,236 | 0.9275× | 0.94× | ✅ |
| curl | 85,638 | 90,481 | 0.9465× | 0.94× | ✅ |
| dns | 230,923 | 232,198 | 0.9945× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
