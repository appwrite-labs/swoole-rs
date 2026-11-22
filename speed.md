# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c9faa18` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,083 | 23,164 | 0.9533× | 0.95× | ✅ |
| service-health | 16,341 | 17,773 | 0.9194× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,986 | 313,839 | 0.6436× | 0.65× | ✅ |
| app | 127,628 | 135,889 | 0.9392× | 0.93× | ✅ |
| http2 | 18,955 | 20,500 | 0.9246× | 0.92× | ✅ |
| table | 194,370 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,086 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,308 | 51,084 | 0.9848× | 0.97× | ✅ |
| http-client | 33,625 | 35,796 | 0.9394× | 0.93× | ✅ |
| gzip | 194,291 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,810 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,281 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,416 | 17,027 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,457 | 409,057 | 0.8910× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,523 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,864 | 41,028 | 0.6304× | 0.64× | ✅ |
| client | 18,621 | 19,726 | 0.9440× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,816 | 24,663 | 0.9251× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,123 | 90,374 | 0.9530× | 0.95× | ✅ |
| dns | 230,219 | 232,982 | 0.9881× | 0.99× | ✅ |
| file | 109,088 | 131,300 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
