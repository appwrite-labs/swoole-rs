# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f65583c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,161 | 0.9535× | 0.95× | ✅ |
| service-health | 16,341 | 17,777 | 0.9192× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,960 | 313,811 | 0.6436× | 0.65× | ✅ |
| app | 127,604 | 135,971 | 0.9385× | 0.93× | ✅ |
| http2 | 18,965 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,303 | 51,088 | 0.9846× | 0.97× | ✅ |
| http-client | 33,661 | 35,793 | 0.9404× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,533 | 18,730 | 0.9361× | 0.93× | ✅ |
| https | 106,313 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,591 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,281 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,415 | 17,024 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,257 | 409,053 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,007 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 41,007 | 0.6303× | 0.64× | ✅ |
| client | 18,618 | 19,730 | 0.9436× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,824 | 24,662 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,138 | 90,423 | 0.9526× | 0.95× | ✅ |
| dns | 229,806 | 232,970 | 0.9864× | 0.99× | ✅ |
| file | 109,096 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
