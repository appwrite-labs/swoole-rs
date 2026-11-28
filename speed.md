# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,047 | 23,173 | 0.9514× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8223× | 0.81× | ✅ |
| timer | 202,179 | 313,929 | 0.6440× | 0.65× | ✅ |
| app | 127,544 | 135,969 | 0.9380× | 0.93× | ✅ |
| http2 | 19,170 | 20,496 | 0.9353× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,023 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,269 | 0.9640× | 0.96× | ✅ |
| mysql | 50,294 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,661 | 35,783 | 0.9407× | 0.93× | ✅ |
| gzip | 194,276 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,988 | 181,565 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,802 | 0.9769× | 0.98× | ✅ |
| wss | 101,570 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,973 | 409,053 | 0.8971× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,692 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,962 | 16,368 | 0.7308× | 0.72× | ✅ |
| upload | 418,892 | 1,322,428 | 0.3168× | 0.32× | ✅ |
| static | 25,370 | 41,005 | 0.6187× | 0.62× | ✅ |
| client | 18,592 | 19,730 | 0.9423× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,888 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,253 | 0.9297× | 0.93× | ✅ |
| curl | 86,079 | 90,422 | 0.9520× | 0.95× | ✅ |
| dns | 230,113 | 232,817 | 0.9884× | 0.99× | ✅ |
| file | 108,880 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
