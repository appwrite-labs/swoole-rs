# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `652b58e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,104 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,786 | 0.9199× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,979 | 313,815 | 0.6436× | 0.65× | ✅ |
| app | 127,633 | 135,957 | 0.9388× | 0.93× | ✅ |
| http2 | 18,966 | 20,493 | 0.9255× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,383 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,650 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,803 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 363,596 | 409,444 | 0.8880× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,535 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,861 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,881 | 21,046 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,835 | 24,662 | 0.9259× | 0.92× | ✅ |
| sqlite | 23,499 | 25,268 | 0.9300× | 0.93× | ✅ |
| curl | 86,147 | 90,418 | 0.9528× | 0.95× | ✅ |
| dns | 230,099 | 233,185 | 0.9868× | 0.99× | ✅ |
| file | 109,104 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
