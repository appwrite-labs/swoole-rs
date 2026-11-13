# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8b4853f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,869 | 15,210 | 0.9118× | 0.92× | ✅ |
| echo-1k | 16,974 | 18,162 | 0.9346× | 0.94× | ✅ |
| json | 21,965 | 23,161 | 0.9484× | 0.95× | ✅ |
| service-health | 16,457 | 17,777 | 0.9258× | 0.93× | ✅ |
| websocket | 6,479 | 8,188 | 0.7913× | 0.81× | ✅ |
| timer | 203,064 | 313,598 | 0.6475× | 0.65× | ✅ |
| app | 125,336 | 135,976 | 0.9217× | 0.93× | ✅ |
| http2 | 18,485 | 20,468 | 0.9031× | 0.92× | ✅ |
| table | 195,431 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,369 | 155,170 | 0.8853× | 0.89× | ✅ |
| task | 31,969 | 33,265 | 0.9611× | 0.96× | ✅ |
| mysql | 49,464 | 51,035 | 0.9692× | 0.97× | ✅ |
| http-client | 33,628 | 35,803 | 0.9393× | 0.94× | ✅ |
| client | 18,405 | 19,757 | 0.9316× | 0.94× | ✅ |
| socket | 19,536 | 21,073 | 0.9271× | 0.92× | ✅ |
| gzip | 194,069 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,306 | 181,568 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,762 | 0.9849× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,375 | 7,986 | 0.9235× | 0.95× | ✅ |
| connect | 17,621 | 18,707 | 0.9420× | 0.98× | ✅ |
| https | 108,040 | 108,810 | 0.9929× | 1.0× | ✅ |
| wss | 103,623 | 104,800 | 0.9888× | 1.0× | ✅ |
| atomic | 49,579 | 51,312 | 0.9662× | 0.97× | ✅ |
| process-mode | 15,034 | 16,960 | 0.8865× | 0.94× | ✅ |
| http2-concurrent | 357,222 | 408,882 | 0.8737× | 0.88× | ✅ |
| websocket-64k | 155,921 | 159,695 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,705 | 16,327 | 0.7781× | 0.79× | ✅ |
| upload | 477,833 | 1,322,386 | 0.3613× | 0.37× | ✅ |
| static | 27,265 | 40,981 | 0.6653× | 0.72× | ✅ |
| pgsql | 23,150 | 24,594 | 0.9413× | 0.95× | ✅ |
| sqlite | 23,410 | 25,236 | 0.9276× | 0.94× | ✅ |
| curl | 85,656 | 90,351 | 0.9480× | 0.94× | ✅ |
| dns | 230,934 | 233,333 | 0.9897× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
