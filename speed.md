# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `131e66c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,730 | 15,214 | 0.9024× | 0.91× | ✅ |
| echo-1k | 16,733 | 18,165 | 0.9212× | 0.92× | ✅ |
| json | 22,060 | 23,164 | 0.9523× | 0.95× | ✅ |
| service-health | 16,318 | 17,777 | 0.9179× | 0.92× | ✅ |
| websocket | 6,671 | 8,189 | 0.8147× | 0.81× | ✅ |
| timer | 201,503 | 313,996 | 0.6417× | 0.65× | ✅ |
| app | 126,973 | 135,977 | 0.9338× | 0.93× | ✅ |
| http2 | 18,964 | 20,500 | 0.9251× | 0.92× | ✅ |
| table | 193,840 | 202,900 | 0.9553× | 0.96× | ✅ |
| channel | 137,340 | 155,717 | 0.8820× | 0.88× | ✅ |
| task | 31,946 | 33,272 | 0.9601× | 0.96× | ✅ |
| mysql | 50,035 | 51,079 | 0.9796× | 0.97× | ✅ |
| http-client | 33,345 | 35,792 | 0.9316× | 0.93× | ✅ |
| gzip | 194,343 | 197,095 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,764 | 0.9752× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 17,563 | 18,734 | 0.9375× | 0.93× | ✅ |
| https | 105,982 | 108,809 | 0.9740× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,162 | 51,285 | 0.9781× | 0.97× | ✅ |
| process-mode | 15,252 | 17,024 | 0.8959× | 0.89× | ✅ |
| http2-concurrent | 366,149 | 409,053 | 0.8951× | 0.89× | ✅ |
| websocket-64k | 89,975 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,838 | 16,372 | 0.7231× | 0.72× | ✅ |
| upload | 416,395 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,030 | 40,999 | 0.6593× | 0.65× | ✅ |
| client | 18,456 | 19,730 | 0.9355× | 0.93× | ✅ |
| socket | 19,683 | 21,045 | 0.9353× | 0.93× | ✅ |
| pgsql | 23,092 | 24,664 | 0.9363× | 0.92× | ✅ |
| sqlite | 23,435 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,928 | 90,550 | 0.9490× | 0.95× | ✅ |
| dns | 229,934 | 232,969 | 0.9870× | 0.99× | ✅ |
| file | 108,206 | 131,300 | 0.8241× | 0.83× | ✅ |

Geometric mean 0.8527×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
