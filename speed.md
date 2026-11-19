# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6f879bf` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,751 | 15,214 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,755 | 18,165 | 0.9224× | 0.92× | ✅ |
| json | 22,065 | 23,173 | 0.9522× | 0.95× | ✅ |
| service-health | 16,326 | 17,786 | 0.9179× | 0.92× | ✅ |
| websocket | 6,671 | 8,185 | 0.8150× | 0.81× | ✅ |
| timer | 201,491 | 313,793 | 0.6421× | 0.65× | ✅ |
| app | 126,998 | 135,960 | 0.9341× | 0.93× | ✅ |
| http2 | 18,968 | 20,500 | 0.9253× | 0.92× | ✅ |
| table | 194,293 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,345 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,961 | 33,272 | 0.9606× | 0.96× | ✅ |
| mysql | 50,037 | 51,088 | 0.9794× | 0.97× | ✅ |
| http-client | 33,351 | 35,796 | 0.9317× | 0.93× | ✅ |
| gzip | 194,358 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,987 | 0.9466× | 0.95× | ✅ |
| connect | 17,588 | 18,734 | 0.9388× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,179 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,263 | 17,021 | 0.8967× | 0.89× | ✅ |
| http2-concurrent | 364,791 | 409,057 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 89,970 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,368 | 0.7233× | 0.72× | ✅ |
| upload | 416,430 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,057 | 41,005 | 0.6598× | 0.65× | ✅ |
| client | 18,468 | 19,730 | 0.9360× | 0.93× | ✅ |
| socket | 19,696 | 21,046 | 0.9359× | 0.93× | ✅ |
| pgsql | 23,099 | 24,663 | 0.9366× | 0.92× | ✅ |
| sqlite | 23,440 | 25,268 | 0.9276× | 0.93× | ✅ |
| curl | 85,929 | 90,549 | 0.9490× | 0.95× | ✅ |
| dns | 229,824 | 232,540 | 0.9883× | 0.99× | ✅ |
| file | 108,213 | 131,300 | 0.8242× | 0.83× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
