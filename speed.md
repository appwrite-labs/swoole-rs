# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,104 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,782 | 0.9201× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,960 | 313,773 | 0.6436× | 0.65× | ✅ |
| app | 127,608 | 135,942 | 0.9387× | 0.93× | ✅ |
| http2 | 18,960 | 20,500 | 0.9249× | 0.92× | ✅ |
| table | 194,327 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,383 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,647 | 35,797 | 0.9399× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9785× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,214 | 51,285 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 363,572 | 409,056 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,539 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9448× | 0.93× | ✅ |
| socket | 19,881 | 21,046 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,831 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,488 | 25,253 | 0.9301× | 0.93× | ✅ |
| curl | 86,137 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 230,088 | 232,893 | 0.9880× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
