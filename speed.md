# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `95eb9f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,161 | 0.9535× | 0.95× | ✅ |
| service-health | 16,341 | 17,773 | 0.9194× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 202,011 | 313,963 | 0.6434× | 0.65× | ✅ |
| app | 127,627 | 135,972 | 0.9386× | 0.93× | ✅ |
| http2 | 18,960 | 20,496 | 0.9251× | 0.92× | ✅ |
| table | 194,374 | 202,904 | 0.9580× | 0.96× | ✅ |
| channel | 137,366 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,091 | 33,272 | 0.9645× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,637 | 35,781 | 0.9401× | 0.93× | ✅ |
| gzip | 194,275 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,802 | 0.9771× | 0.98× | ✅ |
| wss | 101,597 | 104,806 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9054× | 0.89× | ✅ |
| http2-concurrent | 364,386 | 409,487 | 0.8899× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,877 | 40,999 | 0.6312× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,042 | 0.9438× | 0.93× | ✅ |
| pgsql | 22,828 | 24,663 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,131 | 90,422 | 0.9525× | 0.95× | ✅ |
| dns | 229,680 | 232,410 | 0.9883× | 0.99× | ✅ |
| file | 109,099 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
