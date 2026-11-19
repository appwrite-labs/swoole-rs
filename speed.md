# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d5e3da9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,717 | 15,213 | 0.9016× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,165 | 0.9209× | 0.92× | ✅ |
| json | 22,045 | 23,173 | 0.9513× | 0.95× | ✅ |
| service-health | 16,300 | 17,786 | 0.9164× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,442 | 313,911 | 0.6417× | 0.65× | ✅ |
| app | 126,967 | 135,935 | 0.9340× | 0.93× | ✅ |
| http2 | 18,971 | 20,497 | 0.9256× | 0.92× | ✅ |
| table | 194,272 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,324 | 155,723 | 0.8818× | 0.88× | ✅ |
| task | 31,930 | 33,272 | 0.9597× | 0.96× | ✅ |
| mysql | 50,028 | 51,088 | 0.9792× | 0.97× | ✅ |
| http-client | 33,338 | 35,793 | 0.9314× | 0.93× | ✅ |
| gzip | 194,337 | 197,095 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,764 | 0.9753× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 17,563 | 18,730 | 0.9377× | 0.93× | ✅ |
| https | 105,975 | 108,813 | 0.9739× | 0.98× | ✅ |
| wss | 101,212 | 104,800 | 0.9658× | 0.97× | ✅ |
| atomic | 50,158 | 51,285 | 0.9780× | 0.97× | ✅ |
| process-mode | 15,250 | 17,024 | 0.8958× | 0.89× | ✅ |
| http2-concurrent | 365,923 | 409,059 | 0.8945× | 0.89× | ✅ |
| websocket-64k | 89,973 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,838 | 16,372 | 0.7231× | 0.72× | ✅ |
| upload | 416,394 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,059 | 41,028 | 0.6595× | 0.65× | ✅ |
| client | 18,447 | 19,729 | 0.9350× | 0.93× | ✅ |
| socket | 19,676 | 21,045 | 0.9349× | 0.93× | ✅ |
| pgsql | 23,084 | 24,662 | 0.9360× | 0.92× | ✅ |
| sqlite | 23,428 | 25,268 | 0.9272× | 0.93× | ✅ |
| curl | 85,739 | 90,549 | 0.9469× | 0.95× | ✅ |
| dns | 229,930 | 232,923 | 0.9871× | 0.99× | ✅ |
| file | 108,184 | 131,207 | 0.8245× | 0.83× | ✅ |

Geometric mean 0.8528×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
