# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2604e1e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,748 | 15,214 | 0.9037× | 0.91× | ✅ |
| echo-1k | 16,782 | 18,165 | 0.9238× | 0.92× | ✅ |
| json | 22,076 | 23,164 | 0.9530× | 0.95× | ✅ |
| service-health | 16,334 | 17,777 | 0.9188× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8214× | 0.81× | ✅ |
| timer | 202,161 | 313,879 | 0.6441× | 0.65× | ✅ |
| app | 127,551 | 135,971 | 0.9381× | 0.93× | ✅ |
| http2 | 19,162 | 20,497 | 0.9349× | 0.92× | ✅ |
| table | 194,371 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,359 | 155,714 | 0.8821× | 0.88× | ✅ |
| task | 32,110 | 33,269 | 0.9652× | 0.96× | ✅ |
| mysql | 50,319 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,676 | 35,796 | 0.9408× | 0.93× | ✅ |
| gzip | 194,273 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,463 | 181,573 | 0.9884× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,522 | 18,730 | 0.9355× | 0.93× | ✅ |
| https | 106,263 | 108,802 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,797 | 0.9691× | 0.97× | ✅ |
| atomic | 50,191 | 51,281 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,428 | 17,024 | 0.9062× | 0.89× | ✅ |
| http2-concurrent | 367,979 | 409,053 | 0.8996× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,372 | 0.7284× | 0.72× | ✅ |
| upload | 418,884 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,399 | 41,063 | 0.6185× | 0.62× | ✅ |
| client | 18,620 | 19,731 | 0.9437× | 0.93× | ✅ |
| socket | 19,859 | 21,046 | 0.9436× | 0.93× | ✅ |
| pgsql | 22,803 | 24,662 | 0.9246× | 0.92× | ✅ |
| sqlite | 23,503 | 25,253 | 0.9307× | 0.93× | ✅ |
| curl | 86,114 | 90,545 | 0.9511× | 0.95× | ✅ |
| dns | 230,110 | 233,186 | 0.9868× | 0.99× | ✅ |
| file | 108,901 | 131,297 | 0.8294× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
