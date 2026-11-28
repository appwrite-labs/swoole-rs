# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9019× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,051 | 23,170 | 0.9517× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8222× | 0.81× | ✅ |
| timer | 202,190 | 313,787 | 0.6444× | 0.65× | ✅ |
| app | 127,543 | 135,979 | 0.9380× | 0.93× | ✅ |
| http2 | 19,172 | 20,496 | 0.9354× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,023 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,269 | 0.9641× | 0.96× | ✅ |
| mysql | 50,294 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,658 | 35,788 | 0.9405× | 0.93× | ✅ |
| gzip | 194,278 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,565 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,802 | 0.9769× | 0.98× | ✅ |
| wss | 101,566 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,276 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,784 | 408,865 | 0.8971× | 0.89× | ✅ |
| websocket-64k | 90,033 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,897 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,428 | 41,063 | 0.6192× | 0.62× | ✅ |
| client | 18,596 | 19,721 | 0.9430× | 0.93× | ✅ |
| socket | 19,835 | 21,036 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,888 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,072 | 90,546 | 0.9506× | 0.95× | ✅ |
| dns | 230,323 | 232,907 | 0.9889× | 0.99× | ✅ |
| file | 108,880 | 131,296 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
