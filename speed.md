# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0ef21d0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,922 | 314,090 | 0.6429× | 0.65× | ✅ |
| app | 127,642 | 135,940 | 0.9390× | 0.93× | ✅ |
| http2 | 18,971 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,084 | 33,269 | 0.9644× | 0.96× | ✅ |
| mysql | 50,334 | 51,085 | 0.9853× | 0.97× | ✅ |
| http-client | 33,638 | 35,796 | 0.9397× | 0.93× | ✅ |
| gzip | 194,294 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,163 | 181,573 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9785× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,802 | 0.9772× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,215 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,425 | 17,025 | 0.9060× | 0.89× | ✅ |
| http2-concurrent | 363,738 | 408,861 | 0.8896× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,533 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,643 | 19,721 | 0.9454× | 0.93× | ✅ |
| socket | 19,882 | 21,033 | 0.9453× | 0.93× | ✅ |
| pgsql | 22,832 | 24,663 | 0.9258× | 0.92× | ✅ |
| sqlite | 23,532 | 25,268 | 0.9313× | 0.93× | ✅ |
| curl | 86,140 | 90,423 | 0.9526× | 0.95× | ✅ |
| dns | 230,172 | 232,590 | 0.9896× | 0.99× | ✅ |
| file | 109,096 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
