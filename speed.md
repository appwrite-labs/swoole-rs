# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `83c55af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,096 | 23,164 | 0.9539× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,964 | 313,784 | 0.6436× | 0.65× | ✅ |
| app | 127,642 | 135,989 | 0.9386× | 0.93× | ✅ |
| http2 | 18,955 | 20,500 | 0.9246× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,092 | 33,272 | 0.9646× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,674 | 35,793 | 0.9408× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,802 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,434 | 17,028 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 364,431 | 409,053 | 0.8909× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,518 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,902 | 41,028 | 0.6313× | 0.64× | ✅ |
| client | 18,634 | 19,730 | 0.9445× | 0.93× | ✅ |
| socket | 19,870 | 21,045 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,838 | 24,663 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,540 | 25,268 | 0.9316× | 0.93× | ✅ |
| curl | 86,146 | 90,421 | 0.9527× | 0.95× | ✅ |
| dns | 229,925 | 232,728 | 0.9880× | 0.99× | ✅ |
| file | 109,105 | 131,300 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
