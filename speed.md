# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,213 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,105 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,786 | 0.9199× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,975 | 313,797 | 0.6436× | 0.65× | ✅ |
| app | 127,646 | 135,965 | 0.9388× | 0.93× | ✅ |
| http2 | 18,967 | 20,500 | 0.9252× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,383 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,093 | 33,272 | 0.9646× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,678 | 35,784 | 0.9411× | 0.93× | ✅ |
| gzip | 194,280 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9785× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,804 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,438 | 17,027 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 363,398 | 409,053 | 0.8884× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,529 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9448× | 0.93× | ✅ |
| socket | 19,878 | 21,045 | 0.9446× | 0.93× | ✅ |
| pgsql | 22,832 | 24,662 | 0.9258× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,147 | 90,547 | 0.9514× | 0.95× | ✅ |
| dns | 230,349 | 232,535 | 0.9906× | 0.99× | ✅ |
| file | 109,101 | 131,299 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
