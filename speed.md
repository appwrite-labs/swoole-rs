# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8f06bc9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,714 | 15,210 | 0.9016× | 0.91× | ✅ |
| echo-1k | 16,702 | 18,162 | 0.9196× | 0.92× | ✅ |
| json | 22,038 | 23,170 | 0.9512× | 0.95× | ✅ |
| service-health | 16,285 | 17,782 | 0.9158× | 0.92× | ✅ |
| websocket | 6,673 | 8,189 | 0.8148× | 0.8× | ✅ |
| timer | 201,403 | 313,965 | 0.6415× | 0.65× | ✅ |
| app | 126,645 | 135,942 | 0.9316× | 0.93× | ✅ |
| http2 | 18,951 | 20,497 | 0.9246× | 0.92× | ✅ |
| table | 193,874 | 201,885 | 0.9603× | 0.96× | ✅ |
| channel | 137,301 | 155,726 | 0.8817× | 0.88× | ✅ |
| task | 31,910 | 33,272 | 0.9591× | 0.96× | ✅ |
| mysql | 49,737 | 51,064 | 0.9740× | 0.97× | ✅ |
| http-client | 33,544 | 35,796 | 0.9371× | 0.93× | ✅ |
| gzip | 194,308 | 197,095 | 0.9859× | 0.99× | ✅ |
| websocket-deflate | 179,106 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,636 | 4,764 | 0.9732× | 0.96× | ✅ |
| tcp-eof | 5,186 | 10,231 | 0.5069× | 0.5× | ✅ |
| udp | 7,569 | 7,986 | 0.9478× | 0.95× | ✅ |
| connect | 17,525 | 18,734 | 0.9355× | 0.93× | ✅ |
| https | 106,001 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,224 | 104,797 | 0.9659× | 0.97× | ✅ |
| atomic | 50,148 | 51,276 | 0.9780× | 0.97× | ✅ |
| process-mode | 15,218 | 17,024 | 0.8939× | 0.89× | ✅ |
| http2-concurrent | 365,729 | 408,864 | 0.8945× | 0.89× | ✅ |
| websocket-64k | 89,966 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,838 | 16,368 | 0.7232× | 0.72× | ✅ |
| upload | 416,090 | 1,322,425 | 0.3146× | 0.32× | ✅ |
| static | 27,043 | 41,063 | 0.6586× | 0.65× | ✅ |
| client | 18,329 | 19,720 | 0.9295× | 0.93× | ✅ |
| socket | 19,552 | 21,036 | 0.9294× | 0.93× | ✅ |
| pgsql | 22,544 | 24,617 | 0.9158× | 0.92× | ✅ |
| sqlite | 23,376 | 25,236 | 0.9263× | 0.93× | ✅ |
| curl | 85,285 | 90,360 | 0.9438× | 0.95× | ✅ |
| dns | 229,733 | 232,486 | 0.9882× | 0.99× | ✅ |

Geometric mean 0.8522×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
