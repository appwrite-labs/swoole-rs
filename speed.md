# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f5bd50e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,744 | 15,214 | 0.9034× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,166 | 0.9224× | 0.92× | ✅ |
| json | 22,065 | 23,164 | 0.9525× | 0.95× | ✅ |
| service-health | 16,331 | 17,777 | 0.9186× | 0.92× | ✅ |
| websocket | 6,671 | 8,185 | 0.8150× | 0.81× | ✅ |
| timer | 201,567 | 313,936 | 0.6421× | 0.65× | ✅ |
| app | 126,992 | 135,974 | 0.9339× | 0.93× | ✅ |
| http2 | 18,960 | 20,498 | 0.9250× | 0.92× | ✅ |
| table | 193,853 | 202,904 | 0.9554× | 0.96× | ✅ |
| channel | 137,357 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 31,956 | 33,272 | 0.9604× | 0.96× | ✅ |
| mysql | 50,042 | 51,088 | 0.9795× | 0.97× | ✅ |
| http-client | 33,365 | 35,783 | 0.9324× | 0.93× | ✅ |
| gzip | 194,378 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,562 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,987 | 0.9466× | 0.95× | ✅ |
| connect | 17,581 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,004 | 108,813 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,803 | 0.9658× | 0.97× | ✅ |
| atomic | 50,184 | 51,285 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,267 | 17,021 | 0.8970× | 0.89× | ✅ |
| http2-concurrent | 366,290 | 409,057 | 0.8955× | 0.89× | ✅ |
| websocket-64k | 89,970 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,369 | 0.7233× | 0.72× | ✅ |
| upload | 416,436 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,115 | 41,063 | 0.6603× | 0.65× | ✅ |
| client | 18,476 | 19,729 | 0.9365× | 0.93× | ✅ |
| socket | 19,704 | 21,046 | 0.9363× | 0.93× | ✅ |
| pgsql | 23,091 | 24,662 | 0.9363× | 0.92× | ✅ |
| sqlite | 23,429 | 25,268 | 0.9272× | 0.93× | ✅ |
| curl | 85,741 | 90,547 | 0.9469× | 0.95× | ✅ |
| dns | 230,357 | 232,548 | 0.9906× | 0.99× | ✅ |
| file | 108,195 | 130,843 | 0.8269× | 0.83× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
