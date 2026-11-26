# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `93ef4f2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,746 | 15,210 | 0.9037× | 0.91× | ✅ |
| echo-1k | 16,785 | 18,162 | 0.9242× | 0.92× | ✅ |
| json | 22,079 | 23,161 | 0.9533× | 0.95× | ✅ |
| service-health | 16,337 | 17,773 | 0.9192× | 0.92× | ✅ |
| websocket | 6,723 | 8,192 | 0.8207× | 0.81× | ✅ |
| timer | 201,971 | 313,857 | 0.6435× | 0.65× | ✅ |
| app | 127,826 | 135,984 | 0.9400× | 0.93× | ✅ |
| http2 | 19,177 | 20,497 | 0.9356× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 139,689 | 155,714 | 0.8971× | 0.88× | ✅ |
| task | 32,113 | 33,269 | 0.9653× | 0.96× | ✅ |
| mysql | 50,320 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,681 | 35,796 | 0.9409× | 0.93× | ✅ |
| gzip | 194,272 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,987 | 181,570 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,516 | 18,734 | 0.9350× | 0.93× | ✅ |
| https | 106,276 | 108,809 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,450 | 51,285 | 0.9837× | 0.97× | ✅ |
| process-mode | 15,430 | 17,021 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 365,927 | 409,057 | 0.8946× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 418,888 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,229 | 41,092 | 0.6140× | 0.62× | ✅ |
| client | 18,624 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,046 | 0.9436× | 0.93× | ✅ |
| pgsql | 22,927 | 24,663 | 0.9296× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,111 | 90,422 | 0.9523× | 0.95× | ✅ |
| dns | 230,490 | 233,034 | 0.9891× | 0.99× | ✅ |
| file | 108,918 | 131,300 | 0.8295× | 0.83× | ✅ |

Geometric mean 0.8541×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
