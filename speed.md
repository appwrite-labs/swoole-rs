# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,714 | 15,213 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,173 | 0.9515× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,108 | 313,853 | 0.6440× | 0.65× | ✅ |
| app | 127,579 | 135,952 | 0.9384× | 0.93× | ✅ |
| http2 | 19,179 | 20,497 | 0.9357× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,265 | 0.9641× | 0.96× | ✅ |
| mysql | 50,260 | 51,085 | 0.9838× | 0.97× | ✅ |
| http-client | 33,664 | 35,787 | 0.9407× | 0.93× | ✅ |
| gzip | 194,278 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,565 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,764 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,802 | 0.9769× | 0.98× | ✅ |
| wss | 101,566 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,403 | 17,021 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 367,065 | 409,053 | 0.8974× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,896 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,597 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,889 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,073 | 90,549 | 0.9506× | 0.95× | ✅ |
| dns | 229,946 | 232,717 | 0.9881× | 0.99× | ✅ |
| file | 108,876 | 131,210 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
