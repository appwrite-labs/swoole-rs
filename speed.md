# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2ff41d4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,051 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,308 | 17,777 | 0.9174× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,166 | 313,793 | 0.6443× | 0.65× | ✅ |
| app | 127,572 | 135,978 | 0.9382× | 0.93× | ✅ |
| http2 | 19,173 | 20,497 | 0.9354× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,023 | 155,714 | 0.8800× | 0.88× | ✅ |
| task | 32,077 | 33,269 | 0.9642× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,654 | 35,793 | 0.9402× | 0.93× | ✅ |
| gzip | 194,294 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,495 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,802 | 0.9769× | 0.98× | ✅ |
| wss | 101,564 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,413 | 51,285 | 0.9830× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 367,493 | 409,057 | 0.8984× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,695 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,888 | 1,322,430 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,589 | 19,730 | 0.9422× | 0.93× | ✅ |
| socket | 19,828 | 21,046 | 0.9422× | 0.93× | ✅ |
| pgsql | 22,897 | 24,663 | 0.9284× | 0.92× | ✅ |
| sqlite | 23,487 | 25,268 | 0.9295× | 0.93× | ✅ |
| curl | 86,083 | 90,549 | 0.9507× | 0.95× | ✅ |
| dns | 230,365 | 232,982 | 0.9888× | 0.99× | ✅ |
| file | 108,894 | 131,297 | 0.8294× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
