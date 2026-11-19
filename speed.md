# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5376b48` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,740 | 15,214 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,748 | 18,162 | 0.9222× | 0.92× | ✅ |
| json | 22,068 | 23,170 | 0.9525× | 0.95× | ✅ |
| service-health | 16,326 | 17,786 | 0.9179× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,547 | 313,874 | 0.6421× | 0.65× | ✅ |
| app | 126,994 | 136,012 | 0.9337× | 0.93× | ✅ |
| http2 | 18,967 | 20,501 | 0.9252× | 0.92× | ✅ |
| table | 194,289 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,345 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,961 | 33,269 | 0.9607× | 0.96× | ✅ |
| mysql | 50,051 | 51,088 | 0.9797× | 0.97× | ✅ |
| http-client | 33,356 | 35,797 | 0.9318× | 0.93× | ✅ |
| gzip | 194,362 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9467× | 0.95× | ✅ |
| connect | 17,580 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 105,996 | 108,809 | 0.9741× | 0.98× | ✅ |
| wss | 101,212 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,179 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,262 | 17,028 | 0.8963× | 0.89× | ✅ |
| http2-concurrent | 364,970 | 409,053 | 0.8922× | 0.89× | ✅ |
| websocket-64k | 89,974 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,368 | 0.7231× | 0.72× | ✅ |
| upload | 416,428 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,080 | 41,028 | 0.6600× | 0.65× | ✅ |
| client | 18,464 | 19,730 | 0.9358× | 0.93× | ✅ |
| socket | 19,697 | 21,045 | 0.9359× | 0.93× | ✅ |
| pgsql | 23,103 | 24,663 | 0.9367× | 0.92× | ✅ |
| sqlite | 23,449 | 25,268 | 0.9280× | 0.93× | ✅ |
| curl | 85,941 | 90,417 | 0.9505× | 0.95× | ✅ |
| dns | 229,808 | 232,570 | 0.9881× | 0.99× | ✅ |
| file | 108,217 | 131,297 | 0.8242× | 0.83× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
