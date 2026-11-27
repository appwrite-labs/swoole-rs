# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4d4694d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9019× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,170 | 0.9517× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8223× | 0.81× | ✅ |
| timer | 202,209 | 313,769 | 0.6445× | 0.65× | ✅ |
| app | 127,566 | 135,885 | 0.9388× | 0.93× | ✅ |
| http2 | 19,171 | 20,496 | 0.9353× | 0.92× | ✅ |
| table | 194,281 | 201,889 | 0.9623× | 0.96× | ✅ |
| channel | 137,023 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,269 | 0.9640× | 0.96× | ✅ |
| mysql | 50,287 | 51,088 | 0.9843× | 0.97× | ✅ |
| http-client | 33,651 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,495 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,564 | 104,804 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,281 | 0.9833× | 0.97× | ✅ |
| process-mode | 15,403 | 17,021 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 366,982 | 409,149 | 0.8969× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,907 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,358 | 41,005 | 0.6184× | 0.62× | ✅ |
| client | 18,595 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,880 | 24,663 | 0.9277× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,074 | 90,546 | 0.9506× | 0.95× | ✅ |
| dns | 229,923 | 232,990 | 0.9868× | 0.99× | ✅ |
| file | 108,880 | 131,206 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
