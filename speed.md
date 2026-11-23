# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b914ff8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,185 | 0.8212× | 0.81× | ✅ |
| timer | 201,948 | 313,713 | 0.6437× | 0.65× | ✅ |
| app | 127,617 | 136,014 | 0.9383× | 0.93× | ✅ |
| http2 | 18,970 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,627 | 35,794 | 0.9395× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,589 | 104,804 | 0.9693× | 0.97× | ✅ |
| atomic | 50,196 | 51,281 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,415 | 17,021 | 0.9056× | 0.89× | ✅ |
| http2-concurrent | 364,475 | 409,053 | 0.8910× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,860 | 40,999 | 0.6307× | 0.64× | ✅ |
| client | 18,617 | 19,726 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,819 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,516 | 25,268 | 0.9307× | 0.93× | ✅ |
| curl | 86,127 | 90,421 | 0.9525× | 0.95× | ✅ |
| dns | 229,941 | 233,228 | 0.9859× | 0.99× | ✅ |
| file | 109,106 | 130,918 | 0.8334× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
