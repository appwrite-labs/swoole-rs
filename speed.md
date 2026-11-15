# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4a1a2c8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,981 | 15,214 | 0.9190× | 0.92× | ✅ |
| echo-1k | 17,078 | 18,165 | 0.9401× | 0.94× | ✅ |
| json | 22,124 | 23,158 | 0.9554× | 0.95× | ✅ |
| service-health | 16,559 | 17,777 | 0.9315× | 0.93× | ✅ |
| websocket | 6,649 | 8,188 | 0.8121× | 0.8× | ✅ |
| timer | 205,938 | 313,569 | 0.6568× | 0.65× | ✅ |
| app | 125,465 | 135,933 | 0.9230× | 0.93× | ✅ |
| http2 | 18,798 | 20,469 | 0.9184× | 0.91× | ✅ |
| table | 195,578 | 201,865 | 0.9689× | 0.97× | ✅ |
| channel | 137,210 | 155,174 | 0.8842× | 0.88× | ✅ |
| task | 32,524 | 33,268 | 0.9776× | 0.96× | ✅ |
| mysql | 49,589 | 51,018 | 0.9720× | 0.97× | ✅ |
| http-client | 33,832 | 35,800 | 0.9450× | 0.94× | ✅ |
| client | 18,571 | 19,756 | 0.9400× | 0.94× | ✅ |
| socket | 19,691 | 21,072 | 0.9345× | 0.92× | ✅ |
| gzip | 194,184 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,106 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,728 | 4,765 | 0.9922× | 0.99× | ✅ |
| tcp-eof | 5,221 | 10,231 | 0.5103× | 0.51× | ✅ |
| udp | 7,545 | 7,986 | 0.9447× | 0.93× | ✅ |
| connect | 17,776 | 18,707 | 0.9503× | 0.94× | ✅ |
| https | 108,233 | 108,813 | 0.9947× | 1.0× | ✅ |
| wss | 103,874 | 104,794 | 0.9912× | 0.99× | ✅ |
| atomic | 49,720 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 15,112 | 16,960 | 0.8910× | 0.88× | ✅ |
| http2-concurrent | 363,910 | 408,877 | 0.8900× | 0.88× | ✅ |
| websocket-64k | 89,976 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,894 | 16,327 | 0.7285× | 0.73× | ✅ |
| upload | 413,038 | 1,322,387 | 0.3123× | 0.32× | ✅ |
| static | 27,299 | 41,010 | 0.6657× | 0.66× | ✅ |
| pgsql | 23,389 | 24,594 | 0.9510× | 0.95× | ✅ |
| sqlite | 23,558 | 25,236 | 0.9335× | 0.93× | ✅ |
| curl | 86,139 | 90,475 | 0.9521× | 0.94× | ✅ |
| dns | 231,988 | 233,011 | 0.9956× | 0.99× | ✅ |

Geometric mean 0.8588×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
