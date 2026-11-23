# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6a8c6fb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,808 | 18,162 | 0.9254× | 0.92× | ✅ |
| json | 22,101 | 23,161 | 0.9542× | 0.95× | ✅ |
| service-health | 16,350 | 17,773 | 0.9199× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,956 | 313,749 | 0.6437× | 0.65× | ✅ |
| app | 127,631 | 135,939 | 0.9389× | 0.93× | ✅ |
| http2 | 18,962 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,383 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,090 | 33,269 | 0.9646× | 0.96× | ✅ |
| mysql | 50,311 | 51,088 | 0.9848× | 0.97× | ✅ |
| http-client | 33,677 | 35,784 | 0.9411× | 0.93× | ✅ |
| gzip | 194,275 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,764 | 0.9784× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,525 | 18,734 | 0.9355× | 0.93× | ✅ |
| https | 106,296 | 108,813 | 0.9769× | 0.98× | ✅ |
| wss | 101,573 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,410 | 17,028 | 0.9050× | 0.89× | ✅ |
| http2-concurrent | 363,595 | 409,053 | 0.8889× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,506 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,874 | 41,028 | 0.6306× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,046 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,820 | 24,662 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,516 | 25,268 | 0.9307× | 0.93× | ✅ |
| curl | 86,117 | 90,538 | 0.9512× | 0.95× | ✅ |
| dns | 229,580 | 232,973 | 0.9854× | 0.99× | ✅ |
| file | 109,092 | 131,293 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
