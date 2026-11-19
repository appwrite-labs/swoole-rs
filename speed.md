# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9fe25dd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,744 | 15,214 | 0.9034× | 0.91× | ✅ |
| echo-1k | 16,752 | 18,165 | 0.9222× | 0.92× | ✅ |
| json | 22,068 | 23,173 | 0.9523× | 0.95× | ✅ |
| service-health | 16,326 | 17,786 | 0.9179× | 0.92× | ✅ |
| websocket | 6,671 | 8,189 | 0.8147× | 0.81× | ✅ |
| timer | 201,494 | 314,237 | 0.6412× | 0.65× | ✅ |
| app | 126,995 | 135,971 | 0.9340× | 0.93× | ✅ |
| http2 | 18,969 | 20,497 | 0.9255× | 0.92× | ✅ |
| table | 194,293 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,345 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,961 | 33,269 | 0.9607× | 0.96× | ✅ |
| mysql | 50,054 | 51,085 | 0.9798× | 0.97× | ✅ |
| http-client | 33,359 | 35,796 | 0.9319× | 0.93× | ✅ |
| gzip | 194,358 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 17,577 | 18,734 | 0.9382× | 0.93× | ✅ |
| https | 105,996 | 108,809 | 0.9741× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,188 | 51,276 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,266 | 17,024 | 0.8967× | 0.89× | ✅ |
| http2-concurrent | 365,035 | 408,861 | 0.8928× | 0.89× | ✅ |
| websocket-64k | 89,973 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,372 | 0.7229× | 0.72× | ✅ |
| upload | 416,425 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,074 | 41,018 | 0.6601× | 0.65× | ✅ |
| client | 18,474 | 19,720 | 0.9368× | 0.93× | ✅ |
| socket | 19,705 | 21,037 | 0.9367× | 0.93× | ✅ |
| pgsql | 23,113 | 24,663 | 0.9371× | 0.92× | ✅ |
| sqlite | 23,449 | 25,268 | 0.9280× | 0.93× | ✅ |
| curl | 85,939 | 90,421 | 0.9504× | 0.95× | ✅ |
| dns | 229,610 | 233,092 | 0.9851× | 0.99× | ✅ |
| file | 108,217 | 131,207 | 0.8248× | 0.83× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
