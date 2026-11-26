# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `91a1092` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,751 | 15,214 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,781 | 18,166 | 0.9238× | 0.92× | ✅ |
| json | 22,067 | 23,164 | 0.9526× | 0.95× | ✅ |
| service-health | 16,328 | 17,777 | 0.9185× | 0.92× | ✅ |
| websocket | 6,727 | 8,185 | 0.8218× | 0.81× | ✅ |
| timer | 202,211 | 313,880 | 0.6442× | 0.65× | ✅ |
| app | 127,583 | 135,965 | 0.9384× | 0.93× | ✅ |
| http2 | 19,185 | 20,501 | 0.9358× | 0.92× | ✅ |
| table | 194,365 | 202,900 | 0.9579× | 0.96× | ✅ |
| channel | 137,039 | 155,717 | 0.8801× | 0.88× | ✅ |
| task | 32,112 | 33,272 | 0.9651× | 0.96× | ✅ |
| mysql | 50,311 | 51,088 | 0.9848× | 0.97× | ✅ |
| http-client | 33,687 | 35,788 | 0.9413× | 0.93× | ✅ |
| gzip | 194,260 | 197,095 | 0.9856× | 0.99× | ✅ |
| websocket-deflate | 179,987 | 181,562 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,525 | 18,734 | 0.9355× | 0.93× | ✅ |
| https | 106,276 | 108,809 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,797 | 0.9691× | 0.97× | ✅ |
| atomic | 50,443 | 51,285 | 0.9836× | 0.97× | ✅ |
| process-mode | 15,434 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 366,080 | 409,053 | 0.8949× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 418,876 | 1,322,429 | 0.3167× | 0.32× | ✅ |
| static | 25,253 | 41,034 | 0.6154× | 0.62× | ✅ |
| client | 18,617 | 19,730 | 0.9436× | 0.93× | ✅ |
| socket | 19,856 | 21,042 | 0.9436× | 0.93× | ✅ |
| pgsql | 22,917 | 24,663 | 0.9292× | 0.92× | ✅ |
| sqlite | 23,506 | 25,268 | 0.9303× | 0.93× | ✅ |
| curl | 86,107 | 90,538 | 0.9511× | 0.95× | ✅ |
| dns | 230,050 | 232,711 | 0.9886× | 0.99× | ✅ |
| file | 108,904 | 131,297 | 0.8294× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
