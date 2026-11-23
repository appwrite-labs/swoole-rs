# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0d17e41` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,214 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,165 | 0.9532× | 0.95× | ✅ |
| service-health | 16,338 | 17,777 | 0.9190× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,979 | 313,794 | 0.6437× | 0.65× | ✅ |
| app | 127,624 | 135,976 | 0.9386× | 0.93× | ✅ |
| http2 | 18,955 | 20,500 | 0.9246× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,308 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,660 | 35,797 | 0.9403× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,028 | 0.9051× | 0.89× | ✅ |
| http2-concurrent | 364,424 | 409,057 | 0.8909× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,527 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,941 | 41,092 | 0.6313× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,662 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,527 | 25,264 | 0.9312× | 0.93× | ✅ |
| curl | 86,129 | 90,417 | 0.9526× | 0.95× | ✅ |
| dns | 230,439 | 232,597 | 0.9907× | 0.99× | ✅ |
| file | 109,096 | 131,207 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
