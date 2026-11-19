# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `89720a7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,752 | 15,214 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,165 | 0.9224× | 0.92× | ✅ |
| json | 22,073 | 23,173 | 0.9525× | 0.95× | ✅ |
| service-health | 16,331 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,671 | 8,189 | 0.8147× | 0.81× | ✅ |
| timer | 201,480 | 313,740 | 0.6422× | 0.65× | ✅ |
| app | 127,015 | 135,934 | 0.9344× | 0.93× | ✅ |
| http2 | 18,970 | 20,501 | 0.9254× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,964 | 33,269 | 0.9608× | 0.96× | ✅ |
| mysql | 50,050 | 51,088 | 0.9797× | 0.97× | ✅ |
| http-client | 33,375 | 35,796 | 0.9324× | 0.93× | ✅ |
| gzip | 194,366 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,561 | 7,986 | 0.9468× | 0.95× | ✅ |
| connect | 17,585 | 18,734 | 0.9387× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,187 | 51,276 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,275 | 17,021 | 0.8974× | 0.89× | ✅ |
| http2-concurrent | 365,489 | 408,861 | 0.8939× | 0.89× | ✅ |
| websocket-64k | 89,975 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,369 | 0.7231× | 0.72× | ✅ |
| upload | 416,436 | 1,322,428 | 0.3149× | 0.32× | ✅ |
| static | 27,109 | 41,063 | 0.6602× | 0.65× | ✅ |
| client | 18,476 | 19,721 | 0.9369× | 0.93× | ✅ |
| socket | 19,701 | 21,036 | 0.9365× | 0.93× | ✅ |
| pgsql | 23,091 | 24,663 | 0.9362× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,918 | 90,421 | 0.9502× | 0.95× | ✅ |
| dns | 229,707 | 232,841 | 0.9865× | 0.99× | ✅ |
| file | 108,184 | 131,297 | 0.8240× | 0.83× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
