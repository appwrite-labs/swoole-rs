# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e17179e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,166 | 0.9256× | 0.92× | ✅ |
| json | 22,114 | 23,170 | 0.9544× | 0.95× | ✅ |
| service-health | 16,371 | 17,786 | 0.9205× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,403 | 313,963 | 0.6447× | 0.65× | ✅ |
| app | 127,630 | 135,911 | 0.9391× | 0.93× | ✅ |
| http2 | 18,959 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,340 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,396 | 155,726 | 0.8823× | 0.88× | ✅ |
| task | 32,096 | 33,272 | 0.9646× | 0.96× | ✅ |
| mysql | 50,324 | 51,085 | 0.9851× | 0.97× | ✅ |
| http-client | 33,678 | 35,783 | 0.9412× | 0.93× | ✅ |
| gzip | 194,282 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,166 | 181,565 | 0.9868× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,546 | 18,730 | 0.9367× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,800 | 0.9695× | 0.97× | ✅ |
| atomic | 50,226 | 51,285 | 0.9794× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 363,486 | 409,053 | 0.8886× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,369 | 0.7284× | 0.72× | ✅ |
| upload | 416,539 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,861 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,651 | 19,730 | 0.9453× | 0.93× | ✅ |
| socket | 19,890 | 21,045 | 0.9451× | 0.93× | ✅ |
| pgsql | 22,839 | 24,664 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,530 | 25,268 | 0.9312× | 0.93× | ✅ |
| curl | 86,149 | 90,417 | 0.9528× | 0.95× | ✅ |
| dns | 229,846 | 233,140 | 0.9859× | 0.99× | ✅ |
| file | 109,108 | 131,300 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
