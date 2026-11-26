# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `735b511` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,742 | 15,214 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,781 | 18,165 | 0.9238× | 0.92× | ✅ |
| json | 22,079 | 23,164 | 0.9532× | 0.95× | ✅ |
| service-health | 16,334 | 17,777 | 0.9188× | 0.92× | ✅ |
| websocket | 6,727 | 8,189 | 0.8215× | 0.81× | ✅ |
| timer | 202,150 | 313,933 | 0.6439× | 0.65× | ✅ |
| app | 127,575 | 135,978 | 0.9382× | 0.93× | ✅ |
| http2 | 19,182 | 20,497 | 0.9358× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,048 | 155,717 | 0.8801× | 0.88× | ✅ |
| task | 32,099 | 33,272 | 0.9648× | 0.96× | ✅ |
| mysql | 50,320 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,682 | 35,793 | 0.9410× | 0.93× | ✅ |
| gzip | 194,272 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,987 | 181,570 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,513 | 18,734 | 0.9348× | 0.93× | ✅ |
| https | 106,276 | 108,809 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,804 | 0.9690× | 0.97× | ✅ |
| atomic | 50,452 | 51,276 | 0.9839× | 0.97× | ✅ |
| process-mode | 15,434 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 365,880 | 408,865 | 0.8949× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,372 | 0.7284× | 0.72× | ✅ |
| upload | 418,885 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,229 | 41,092 | 0.6140× | 0.62× | ✅ |
| client | 18,628 | 19,720 | 0.9446× | 0.93× | ✅ |
| socket | 19,865 | 21,037 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,926 | 24,663 | 0.9296× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,106 | 90,422 | 0.9523× | 0.95× | ✅ |
| dns | 229,868 | 232,780 | 0.9875× | 0.99× | ✅ |
| file | 108,922 | 131,300 | 0.8296× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
