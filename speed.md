# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5bde607` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,213 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,092 | 23,164 | 0.9537× | 0.95× | ✅ |
| service-health | 16,358 | 17,777 | 0.9202× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,974 | 313,972 | 0.6433× | 0.65× | ✅ |
| app | 127,597 | 136,006 | 0.9382× | 0.93× | ✅ |
| http2 | 19,092 | 20,500 | 0.9313× | 0.92× | ✅ |
| table | 194,390 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,382 | 155,717 | 0.8823× | 0.88× | ✅ |
| task | 32,086 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,347 | 51,085 | 0.9856× | 0.97× | ✅ |
| http-client | 33,650 | 35,796 | 0.9400× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,211 | 181,573 | 0.9870× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,578 | 104,804 | 0.9692× | 0.97× | ✅ |
| atomic | 50,203 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,431 | 17,027 | 0.9062× | 0.89× | ✅ |
| http2-concurrent | 366,282 | 409,057 | 0.8954× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,517 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,862 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,639 | 19,730 | 0.9447× | 0.93× | ✅ |
| socket | 19,878 | 21,046 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,827 | 24,663 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,135 | 90,539 | 0.9514× | 0.95× | ✅ |
| dns | 230,180 | 232,831 | 0.9886× | 0.99× | ✅ |
| file | 108,940 | 131,210 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
