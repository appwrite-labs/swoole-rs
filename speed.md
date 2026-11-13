# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7f1f4b2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,874 | 15,213 | 0.9120× | 0.92× | ✅ |
| echo-1k | 16,965 | 18,165 | 0.9340× | 0.94× | ✅ |
| json | 21,949 | 23,161 | 0.9477× | 0.95× | ✅ |
| service-health | 16,451 | 17,777 | 0.9254× | 0.93× | ✅ |
| websocket | 6,476 | 8,188 | 0.7909× | 0.81× | ✅ |
| timer | 202,989 | 313,574 | 0.6473× | 0.65× | ✅ |
| app | 125,314 | 135,948 | 0.9218× | 0.93× | ✅ |
| http2 | 18,480 | 20,468 | 0.9029× | 0.92× | ✅ |
| table | 195,425 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,367 | 154,655 | 0.8882× | 0.89× | ✅ |
| task | 31,908 | 33,268 | 0.9591× | 0.96× | ✅ |
| mysql | 49,467 | 51,064 | 0.9687× | 0.97× | ✅ |
| http-client | 33,619 | 35,809 | 0.9388× | 0.94× | ✅ |
| client | 18,403 | 19,758 | 0.9314× | 0.94× | ✅ |
| socket | 19,534 | 21,067 | 0.9272× | 0.92× | ✅ |
| gzip | 194,064 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,261 | 181,568 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,765 | 0.9842× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,375 | 7,986 | 0.9235× | 0.95× | ✅ |
| connect | 17,544 | 18,703 | 0.9380× | 0.98× | ✅ |
| https | 108,030 | 108,813 | 0.9928× | 1.0× | ✅ |
| wss | 103,623 | 104,794 | 0.9888× | 1.0× | ✅ |
| atomic | 49,572 | 51,316 | 0.9660× | 0.97× | ✅ |
| process-mode | 15,025 | 16,963 | 0.8858× | 0.94× | ✅ |
| http2-concurrent | 357,298 | 408,882 | 0.8738× | 0.88× | ✅ |
| websocket-64k | 155,921 | 159,699 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,704 | 16,331 | 0.7779× | 0.79× | ✅ |
| upload | 477,808 | 1,322,381 | 0.3613× | 0.37× | ✅ |
| static | 27,307 | 41,010 | 0.6659× | 0.72× | ✅ |
| pgsql | 23,145 | 24,594 | 0.9411× | 0.95× | ✅ |
| sqlite | 23,415 | 25,236 | 0.9278× | 0.94× | ✅ |
| curl | 85,650 | 90,350 | 0.9480× | 0.94× | ✅ |
| dns | 230,586 | 232,762 | 0.9906× | 0.99× | ✅ |

Geometric mean 0.8722×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
