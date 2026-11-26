# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `29bd02b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,731 | 15,214 | 0.9025× | 0.91× | ✅ |
| echo-1k | 16,779 | 18,162 | 0.9238× | 0.92× | ✅ |
| json | 22,066 | 23,170 | 0.9524× | 0.95× | ✅ |
| service-health | 16,324 | 17,786 | 0.9178× | 0.92× | ✅ |
| websocket | 6,727 | 8,192 | 0.8212× | 0.81× | ✅ |
| timer | 201,947 | 313,916 | 0.6433× | 0.65× | ✅ |
| app | 127,584 | 135,926 | 0.9386× | 0.93× | ✅ |
| http2 | 19,178 | 20,497 | 0.9356× | 0.92× | ✅ |
| table | 194,289 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,345 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 32,094 | 33,269 | 0.9647× | 0.96× | ✅ |
| mysql | 50,301 | 51,085 | 0.9847× | 0.97× | ✅ |
| http-client | 33,670 | 35,796 | 0.9406× | 0.93× | ✅ |
| gzip | 194,266 | 197,099 | 0.9856× | 0.99× | ✅ |
| websocket-deflate | 179,468 | 181,573 | 0.9884× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,506 | 18,734 | 0.9345× | 0.93× | ✅ |
| https | 106,262 | 108,813 | 0.9766× | 0.98× | ✅ |
| wss | 101,558 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,179 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,415 | 17,028 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 367,898 | 409,056 | 0.8994× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,926 | 16,372 | 0.7284× | 0.72× | ✅ |
| upload | 416,437 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 25,373 | 41,067 | 0.6178× | 0.64× | ✅ |
| client | 18,608 | 19,730 | 0.9431× | 0.93× | ✅ |
| socket | 19,848 | 21,045 | 0.9431× | 0.93× | ✅ |
| pgsql | 22,792 | 24,663 | 0.9242× | 0.92× | ✅ |
| sqlite | 23,488 | 25,264 | 0.9297× | 0.93× | ✅ |
| curl | 86,103 | 90,417 | 0.9523× | 0.95× | ✅ |
| dns | 230,483 | 232,659 | 0.9906× | 0.99× | ✅ |
| file | 108,913 | 131,296 | 0.8295× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
