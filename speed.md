# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `afaf323` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,162 | 0.9258× | 0.92× | ✅ |
| json | 22,096 | 23,161 | 0.9540× | 0.95× | ✅ |
| service-health | 16,354 | 17,773 | 0.9202× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,929 | 314,113 | 0.6429× | 0.65× | ✅ |
| app | 127,623 | 135,851 | 0.9394× | 0.93× | ✅ |
| http2 | 18,961 | 20,496 | 0.9251× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,325 | 51,076 | 0.9853× | 0.97× | ✅ |
| http-client | 33,649 | 35,797 | 0.9400× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,764 | 0.9784× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,336 | 108,809 | 0.9773× | 0.98× | ✅ |
| wss | 101,589 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,214 | 51,285 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 363,550 | 409,053 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,525 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,881 | 21,046 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,831 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,264 | 0.9298× | 0.93× | ✅ |
| curl | 86,137 | 90,537 | 0.9514× | 0.95× | ✅ |
| dns | 229,810 | 232,563 | 0.9882× | 0.99× | ✅ |
| file | 109,101 | 131,207 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
