# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0b88c42` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,716 | 15,213 | 0.9016× | 0.91× | ✅ |
| echo-1k | 16,724 | 18,165 | 0.9207× | 0.92× | ✅ |
| json | 22,046 | 23,170 | 0.9515× | 0.95× | ✅ |
| service-health | 16,303 | 17,786 | 0.9166× | 0.92× | ✅ |
| websocket | 6,675 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,436 | 313,791 | 0.6419× | 0.65× | ✅ |
| app | 126,983 | 136,001 | 0.9337× | 0.93× | ✅ |
| http2 | 18,969 | 20,496 | 0.9255× | 0.92× | ✅ |
| table | 194,272 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,324 | 155,726 | 0.8818× | 0.88× | ✅ |
| task | 31,937 | 33,269 | 0.9600× | 0.96× | ✅ |
| mysql | 50,019 | 51,086 | 0.9791× | 0.97× | ✅ |
| http-client | 33,349 | 35,783 | 0.9320× | 0.93× | ✅ |
| gzip | 194,349 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,562 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,987 | 0.9465× | 0.95× | ✅ |
| connect | 17,563 | 18,734 | 0.9375× | 0.93× | ✅ |
| https | 105,975 | 108,802 | 0.9740× | 0.98× | ✅ |
| wss | 101,212 | 104,800 | 0.9658× | 0.97× | ✅ |
| atomic | 50,158 | 51,285 | 0.9780× | 0.97× | ✅ |
| process-mode | 15,241 | 17,024 | 0.8953× | 0.89× | ✅ |
| http2-concurrent | 365,284 | 409,057 | 0.8930× | 0.89× | ✅ |
| websocket-64k | 89,969 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,842 | 16,368 | 0.7235× | 0.72× | ✅ |
| upload | 416,412 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,044 | 40,999 | 0.6596× | 0.65× | ✅ |
| client | 18,450 | 19,730 | 0.9351× | 0.93× | ✅ |
| socket | 19,676 | 21,046 | 0.9349× | 0.93× | ✅ |
| pgsql | 23,084 | 24,663 | 0.9360× | 0.92× | ✅ |
| sqlite | 23,428 | 25,268 | 0.9272× | 0.93× | ✅ |
| curl | 85,917 | 90,421 | 0.9502× | 0.95× | ✅ |
| dns | 229,247 | 232,481 | 0.9861× | 0.99× | ✅ |
| file | 108,192 | 131,211 | 0.8246× | 0.83× | ✅ |

Geometric mean 0.8528×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
