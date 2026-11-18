# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `aaf69e4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,714 | 15,214 | 0.9014× | 0.91× | ✅ |
| echo-1k | 16,698 | 18,165 | 0.9192× | 0.93× | ✅ |
| json | 22,043 | 23,161 | 0.9517× | 0.95× | ✅ |
| service-health | 16,290 | 17,777 | 0.9163× | 0.92× | ✅ |
| websocket | 6,669 | 8,188 | 0.8145× | 0.8× | ✅ |
| timer | 201,381 | 314,117 | 0.6411× | 0.65× | ✅ |
| app | 126,564 | 135,972 | 0.9308× | 0.93× | ✅ |
| http2 | 18,925 | 20,498 | 0.9233× | 0.92× | ✅ |
| table | 193,750 | 201,876 | 0.9597× | 0.96× | ✅ |
| channel | 137,305 | 155,718 | 0.8818× | 0.88× | ✅ |
| task | 31,904 | 33,265 | 0.9591× | 0.96× | ✅ |
| mysql | 49,742 | 51,064 | 0.9741× | 0.97× | ✅ |
| http-client | 33,552 | 35,800 | 0.9372× | 0.93× | ✅ |
| gzip | 194,308 | 197,093 | 0.9859× | 0.99× | ✅ |
| websocket-deflate | 179,106 | 181,569 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,652 | 4,763 | 0.9768× | 0.96× | ✅ |
| tcp-eof | 5,186 | 10,231 | 0.5069× | 0.5× | ✅ |
| udp | 7,569 | 7,986 | 0.9478× | 0.95× | ✅ |
| connect | 17,513 | 18,686 | 0.9372× | 0.93× | ✅ |
| https | 106,002 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,228 | 104,794 | 0.9660× | 0.97× | ✅ |
| atomic | 50,152 | 51,276 | 0.9781× | 0.97× | ✅ |
| process-mode | 15,221 | 16,959 | 0.8975× | 0.89× | ✅ |
| http2-concurrent | 364,521 | 409,069 | 0.8911× | 0.89× | ✅ |
| websocket-64k | 89,966 | 159,690 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,838 | 16,364 | 0.7234× | 0.72× | ✅ |
| upload | 416,079 | 1,322,413 | 0.3146× | 0.32× | ✅ |
| static | 27,049 | 41,024 | 0.6593× | 0.65× | ✅ |
| client | 18,334 | 19,757 | 0.9280× | 0.93× | ✅ |
| socket | 19,550 | 21,072 | 0.9278× | 0.93× | ✅ |
| pgsql | 22,544 | 24,594 | 0.9166× | 0.92× | ✅ |
| sqlite | 23,376 | 25,221 | 0.9268× | 0.93× | ✅ |
| curl | 85,290 | 90,351 | 0.9440× | 0.95× | ✅ |
| dns | 229,182 | 233,098 | 0.9832× | 0.99× | ✅ |

Geometric mean 0.8522×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
