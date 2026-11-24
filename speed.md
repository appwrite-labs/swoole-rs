# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d8c03a3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,804 | 18,165 | 0.9251× | 0.92× | ✅ |
| json | 22,100 | 23,164 | 0.9541× | 0.95× | ✅ |
| service-health | 16,350 | 17,777 | 0.9197× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,028 | 313,793 | 0.6438× | 0.65× | ✅ |
| app | 127,623 | 135,946 | 0.9388× | 0.93× | ✅ |
| http2 | 18,960 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,383 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,085 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,082 | 0.9851× | 0.97× | ✅ |
| http-client | 33,621 | 35,793 | 0.9393× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,522 | 18,734 | 0.9353× | 0.93× | ✅ |
| https | 106,296 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,575 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,412 | 17,027 | 0.9051× | 0.89× | ✅ |
| http2-concurrent | 363,598 | 409,053 | 0.8889× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,520 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,898 | 41,063 | 0.6307× | 0.64× | ✅ |
| client | 18,623 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,042 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,814 | 24,663 | 0.9250× | 0.92× | ✅ |
| sqlite | 23,516 | 25,268 | 0.9307× | 0.93× | ✅ |
| curl | 86,126 | 90,546 | 0.9512× | 0.95× | ✅ |
| dns | 229,972 | 232,704 | 0.9883× | 0.99× | ✅ |
| file | 109,084 | 131,300 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
