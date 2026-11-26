# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e828966` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,714 | 15,210 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,043 | 23,170 | 0.9514× | 0.95× | ✅ |
| service-health | 16,301 | 17,782 | 0.9167× | 0.92× | ✅ |
| websocket | 6,740 | 8,192 | 0.8227× | 0.81× | ✅ |
| timer | 202,258 | 313,779 | 0.6446× | 0.65× | ✅ |
| app | 127,544 | 135,879 | 0.9387× | 0.93× | ✅ |
| http2 | 19,187 | 20,497 | 0.9361× | 0.92× | ✅ |
| table | 194,274 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,012 | 155,723 | 0.8798× | 0.88× | ✅ |
| task | 32,076 | 33,268 | 0.9642× | 0.96× | ✅ |
| mysql | 50,294 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,650 | 35,795 | 0.9401× | 0.93× | ✅ |
| gzip | 194,294 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,730 | 0.9340× | 0.93× | ✅ |
| https | 106,288 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,570 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,416 | 51,285 | 0.9831× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 367,393 | 409,053 | 0.8982× | 0.89× | ✅ |
| websocket-64k | 90,035 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,899 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,349 | 41,034 | 0.6177× | 0.62× | ✅ |
| client | 18,589 | 19,729 | 0.9422× | 0.93× | ✅ |
| socket | 19,828 | 21,046 | 0.9421× | 0.93× | ✅ |
| pgsql | 22,888 | 24,662 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,075 | 90,416 | 0.9520× | 0.95× | ✅ |
| dns | 230,117 | 232,839 | 0.9883× | 0.99× | ✅ |
| file | 108,885 | 131,207 | 0.8299× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
