# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3e7b42c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,084 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,725 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,863 | 313,791 | 0.6433× | 0.65× | ✅ |
| app | 127,630 | 135,995 | 0.9385× | 0.93× | ✅ |
| http2 | 18,965 | 20,497 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,269 | 0.9644× | 0.96× | ✅ |
| mysql | 50,312 | 51,086 | 0.9848× | 0.97× | ✅ |
| http-client | 33,625 | 35,793 | 0.9394× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,802 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,257 | 409,059 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,924 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,618 | 19,729 | 0.9436× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,817 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,126 | 90,423 | 0.9525× | 0.95× | ✅ |
| dns | 229,761 | 232,845 | 0.9868× | 0.99× | ✅ |
| file | 109,088 | 131,211 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
