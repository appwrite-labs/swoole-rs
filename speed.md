# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3102957` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,763 | 15,214 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,165 | 0.9248× | 0.92× | ✅ |
| json | 22,084 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,342 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,994 | 313,785 | 0.6437× | 0.65× | ✅ |
| app | 127,593 | 135,961 | 0.9385× | 0.93× | ✅ |
| http2 | 18,985 | 20,500 | 0.9261× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,320 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,669 | 35,788 | 0.9408× | 0.93× | ✅ |
| gzip | 194,281 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,533 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,312 | 108,806 | 0.9771× | 0.98× | ✅ |
| wss | 101,594 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,200 | 51,272 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,429 | 17,021 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 364,331 | 408,861 | 0.8911× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,369 | 0.7285× | 0.72× | ✅ |
| upload | 416,512 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,926 | 41,063 | 0.6314× | 0.64× | ✅ |
| client | 18,627 | 19,721 | 0.9445× | 0.93× | ✅ |
| socket | 19,866 | 21,033 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,824 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,520 | 25,253 | 0.9314× | 0.93× | ✅ |
| curl | 86,135 | 90,399 | 0.9528× | 0.95× | ✅ |
| dns | 229,973 | 232,783 | 0.9879× | 0.99× | ✅ |
| file | 108,942 | 131,211 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
