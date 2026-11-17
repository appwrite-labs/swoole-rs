# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e47e0d6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,751 | 15,214 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,720 | 18,165 | 0.9205× | 0.93× | ✅ |
| json | 22,049 | 23,161 | 0.9520× | 0.95× | ✅ |
| service-health | 16,320 | 17,777 | 0.9181× | 0.92× | ✅ |
| websocket | 6,654 | 8,188 | 0.8127× | 0.8× | ✅ |
| timer | 201,961 | 313,949 | 0.6433× | 0.65× | ✅ |
| app | 126,347 | 135,945 | 0.9294× | 0.93× | ✅ |
| http2 | 18,867 | 20,497 | 0.9205× | 0.92× | ✅ |
| table | 193,911 | 201,876 | 0.9605× | 0.96× | ✅ |
| channel | 139,450 | 155,717 | 0.8955× | 0.88× | ✅ |
| task | 31,942 | 33,268 | 0.9601× | 0.96× | ✅ |
| mysql | 49,746 | 51,064 | 0.9742× | 0.97× | ✅ |
| http-client | 33,609 | 35,802 | 0.9387× | 0.93× | ✅ |
| gzip | 194,196 | 197,093 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,073 | 181,569 | 0.9863× | 0.99× | ✅ |
| tcp-length | 4,598 | 4,763 | 0.9654× | 0.96× | ✅ |
| tcp-eof | 5,096 | 10,231 | 0.4981× | 0.5× | ✅ |
| udp | 7,550 | 7,987 | 0.9453× | 0.95× | ✅ |
| connect | 17,302 | 18,686 | 0.9260× | 0.93× | ✅ |
| https | 106,011 | 108,809 | 0.9743× | 0.98× | ✅ |
| wss | 101,200 | 104,794 | 0.9657× | 0.97× | ✅ |
| atomic | 49,790 | 51,276 | 0.9710× | 0.97× | ✅ |
| process-mode | 15,219 | 16,959 | 0.8974× | 0.89× | ✅ |
| http2-concurrent | 363,812 | 408,877 | 0.8898× | 0.9× | ✅ |
| websocket-64k | 89,936 | 159,690 | 0.5632× | 0.57× | ✅ |
| websocket-fragmented | 11,786 | 16,368 | 0.7201× | 0.72× | ✅ |
| upload | 415,984 | 1,322,413 | 0.3146× | 0.32× | ✅ |
| static | 26,760 | 40,989 | 0.6528× | 0.65× | ✅ |
| client | 18,365 | 19,757 | 0.9295× | 0.94× | ✅ |
| socket | 19,585 | 21,073 | 0.9294× | 0.93× | ✅ |
| pgsql | 22,586 | 24,594 | 0.9183× | 0.93× | ✅ |
| sqlite | 23,410 | 25,236 | 0.9277× | 0.93× | ✅ |
| curl | 84,801 | 90,350 | 0.9386× | 0.95× | ✅ |
| dns | 230,193 | 232,932 | 0.9882× | 0.99× | ✅ |

Geometric mean 0.8511×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
