# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,214 | 0.9049× | 0.91× | ✅ |
| echo-1k | 16,803 | 18,165 | 0.9250× | 0.92× | ✅ |
| json | 22,105 | 23,161 | 0.9544× | 0.95× | ✅ |
| service-health | 16,362 | 17,773 | 0.9206× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 201,991 | 313,967 | 0.6434× | 0.65× | ✅ |
| app | 127,540 | 135,976 | 0.9380× | 0.93× | ✅ |
| http2 | 19,002 | 20,496 | 0.9271× | 0.92× | ✅ |
| table | 194,395 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,383 | 155,717 | 0.8823× | 0.88× | ✅ |
| task | 32,088 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,280 | 51,088 | 0.9842× | 0.97× | ✅ |
| http-client | 33,615 | 35,784 | 0.9394× | 0.93× | ✅ |
| gzip | 194,289 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,618 | 7,986 | 0.9539× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,300 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,214 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,433 | 17,025 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 364,064 | 408,864 | 0.8904× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,512 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,620 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,867 | 21,036 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,818 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,143 | 90,547 | 0.9514× | 0.95× | ✅ |
| dns | 230,100 | 232,923 | 0.9879× | 0.99× | ✅ |
| file | 108,941 | 131,297 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
