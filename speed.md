# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `25645fe` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,740 | 15,213 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,749 | 18,166 | 0.9220× | 0.92× | ✅ |
| json | 22,061 | 23,164 | 0.9524× | 0.95× | ✅ |
| service-health | 16,319 | 17,777 | 0.9180× | 0.92× | ✅ |
| websocket | 6,670 | 8,189 | 0.8145× | 0.81× | ✅ |
| timer | 201,612 | 313,860 | 0.6424× | 0.65× | ✅ |
| app | 127,071 | 135,897 | 0.9351× | 0.93× | ✅ |
| http2 | 18,971 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 193,907 | 202,900 | 0.9557× | 0.96× | ✅ |
| channel | 137,973 | 155,717 | 0.8860× | 0.88× | ✅ |
| task | 31,943 | 33,272 | 0.9601× | 0.96× | ✅ |
| mysql | 50,059 | 51,085 | 0.9799× | 0.97× | ✅ |
| http-client | 33,373 | 35,796 | 0.9323× | 0.93× | ✅ |
| gzip | 194,367 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,101 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,642 | 4,763 | 0.9746× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,570 | 7,987 | 0.9478× | 0.95× | ✅ |
| connect | 17,581 | 18,734 | 0.9385× | 0.93× | ✅ |
| https | 105,995 | 108,809 | 0.9741× | 0.98× | ✅ |
| wss | 101,215 | 104,806 | 0.9657× | 0.97× | ✅ |
| atomic | 50,177 | 51,272 | 0.9786× | 0.97× | ✅ |
| process-mode | 15,265 | 17,024 | 0.8966× | 0.89× | ✅ |
| http2-concurrent | 364,726 | 408,861 | 0.8921× | 0.89× | ✅ |
| websocket-64k | 89,976 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,834 | 16,368 | 0.7230× | 0.72× | ✅ |
| upload | 416,457 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,045 | 40,999 | 0.6596× | 0.65× | ✅ |
| client | 18,479 | 19,721 | 0.9370× | 0.93× | ✅ |
| socket | 19,704 | 21,033 | 0.9368× | 0.93× | ✅ |
| pgsql | 23,019 | 24,663 | 0.9334× | 0.92× | ✅ |
| sqlite | 23,449 | 25,268 | 0.9280× | 0.93× | ✅ |
| curl | 85,430 | 90,422 | 0.9448× | 0.95× | ✅ |
| dns | 229,605 | 232,634 | 0.9870× | 0.99× | ✅ |
| file | 108,398 | 131,300 | 0.8256× | 1.1× | ✅ |

Geometric mean 0.8529×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
