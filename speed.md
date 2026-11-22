# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `dfe3634` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,341 | 17,782 | 0.9190× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,909 | 313,805 | 0.6434× | 0.65× | ✅ |
| app | 127,675 | 135,936 | 0.9392× | 0.93× | ✅ |
| http2 | 18,958 | 20,499 | 0.9248× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,084 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,657 | 35,792 | 0.9404× | 0.93× | ✅ |
| gzip | 194,287 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,532 | 18,730 | 0.9360× | 0.93× | ✅ |
| https | 106,313 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,594 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,272 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,413 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,536 | 408,865 | 0.8916× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,528 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,854 | 41,005 | 0.6305× | 0.64× | ✅ |
| client | 18,621 | 19,717 | 0.9444× | 0.93× | ✅ |
| socket | 19,860 | 21,036 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,828 | 24,662 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,527 | 25,253 | 0.9316× | 0.93× | ✅ |
| curl | 86,130 | 90,552 | 0.9512× | 0.95× | ✅ |
| dns | 229,973 | 232,834 | 0.9877× | 0.99× | ✅ |
| file | 109,096 | 131,211 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
