# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b914ff8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,164 | 0.9533× | 0.95× | ✅ |
| service-health | 16,341 | 17,773 | 0.9194× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 201,928 | 313,889 | 0.6433× | 0.65× | ✅ |
| app | 127,643 | 135,944 | 0.9389× | 0.93× | ✅ |
| http2 | 18,959 | 20,497 | 0.9250× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,366 | 155,718 | 0.8821× | 0.88× | ✅ |
| task | 32,086 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,083 | 0.9848× | 0.97× | ✅ |
| http-client | 33,629 | 35,793 | 0.9395× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,806 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,198 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,509 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,889 | 40,999 | 0.6314× | 0.64× | ✅ |
| client | 18,622 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,857 | 21,042 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,817 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,512 | 25,268 | 0.9305× | 0.93× | ✅ |
| curl | 86,132 | 90,417 | 0.9526× | 0.95× | ✅ |
| dns | 229,924 | 233,003 | 0.9868× | 0.99× | ✅ |
| file | 109,106 | 131,300 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
