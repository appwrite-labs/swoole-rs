# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3e734e8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,213 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,043 | 23,170 | 0.9514× | 0.95× | ✅ |
| service-health | 16,301 | 17,786 | 0.9165× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,181 | 313,905 | 0.6441× | 0.65× | ✅ |
| app | 127,539 | 135,980 | 0.9379× | 0.93× | ✅ |
| http2 | 19,175 | 20,498 | 0.9355× | 0.92× | ✅ |
| table | 194,274 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,016 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,265 | 0.9643× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,650 | 35,797 | 0.9400× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,276 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,407 | 17,021 | 0.9052× | 0.89× | ✅ |
| http2-concurrent | 367,240 | 408,861 | 0.8982× | 0.89× | ✅ |
| websocket-64k | 90,036 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,428 | 0.3168× | 0.32× | ✅ |
| static | 25,362 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,596 | 19,721 | 0.9429× | 0.93× | ✅ |
| socket | 19,835 | 21,036 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,896 | 24,658 | 0.9285× | 0.92× | ✅ |
| sqlite | 23,487 | 25,268 | 0.9295× | 0.93× | ✅ |
| curl | 86,086 | 90,417 | 0.9521× | 0.95× | ✅ |
| dns | 229,892 | 232,778 | 0.9876× | 0.99× | ✅ |
| file | 108,894 | 131,296 | 0.8294× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
