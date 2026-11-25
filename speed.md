# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `de8eaea` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,732 | 15,213 | 0.9026× | 0.91× | ✅ |
| echo-1k | 16,775 | 18,162 | 0.9237× | 0.92× | ✅ |
| json | 22,068 | 23,161 | 0.9528× | 0.95× | ✅ |
| service-health | 16,325 | 17,777 | 0.9183× | 0.92× | ✅ |
| websocket | 6,728 | 8,189 | 0.8216× | 0.81× | ✅ |
| timer | 202,028 | 313,989 | 0.6434× | 0.65× | ✅ |
| app | 127,567 | 135,852 | 0.9390× | 0.93× | ✅ |
| http2 | 19,170 | 20,497 | 0.9353× | 0.92× | ✅ |
| table | 194,358 | 202,900 | 0.9579× | 0.96× | ✅ |
| channel | 137,346 | 155,717 | 0.8820× | 0.88× | ✅ |
| task | 32,078 | 33,269 | 0.9642× | 0.96× | ✅ |
| mysql | 50,301 | 51,085 | 0.9847× | 0.97× | ✅ |
| http-client | 33,686 | 35,787 | 0.9413× | 0.93× | ✅ |
| gzip | 194,262 | 197,095 | 0.9856× | 0.99× | ✅ |
| websocket-deflate | 179,300 | 181,565 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,507 | 18,734 | 0.9345× | 0.93× | ✅ |
| https | 106,282 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,584 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,173 | 51,285 | 0.9783× | 0.97× | ✅ |
| process-mode | 15,406 | 17,024 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 367,849 | 409,053 | 0.8993× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,926 | 16,368 | 0.7286× | 0.72× | ✅ |
| upload | 416,427 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 25,846 | 40,999 | 0.6304× | 0.64× | ✅ |
| client | 18,606 | 19,730 | 0.9430× | 0.93× | ✅ |
| socket | 19,845 | 21,045 | 0.9430× | 0.93× | ✅ |
| pgsql | 22,802 | 24,662 | 0.9246× | 0.92× | ✅ |
| sqlite | 23,501 | 25,268 | 0.9301× | 0.93× | ✅ |
| curl | 86,108 | 90,550 | 0.9509× | 0.95× | ✅ |
| dns | 230,161 | 232,710 | 0.9890× | 0.99× | ✅ |
| file | 108,913 | 130,921 | 0.8319× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
