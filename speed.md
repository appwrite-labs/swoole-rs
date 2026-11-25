# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e5008f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,742 | 15,210 | 0.9035× | 0.91× | ✅ |
| echo-1k | 16,780 | 18,165 | 0.9237× | 0.92× | ✅ |
| json | 22,065 | 23,173 | 0.9522× | 0.95× | ✅ |
| service-health | 16,323 | 17,786 | 0.9178× | 0.92× | ✅ |
| websocket | 6,724 | 8,192 | 0.8208× | 0.81× | ✅ |
| timer | 202,006 | 313,880 | 0.6436× | 0.65× | ✅ |
| app | 127,576 | 135,970 | 0.9383× | 0.93× | ✅ |
| http2 | 19,076 | 20,496 | 0.9307× | 0.92× | ✅ |
| table | 194,288 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,348 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9640× | 0.96× | ✅ |
| mysql | 50,311 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,702 | 35,795 | 0.9415× | 0.93× | ✅ |
| gzip | 194,278 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,300 | 181,570 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,513 | 18,734 | 0.9348× | 0.93× | ✅ |
| https | 106,281 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,584 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,178 | 51,281 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,403 | 17,021 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 366,195 | 409,057 | 0.8952× | 0.89× | ✅ |
| websocket-64k | 90,011 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,926 | 16,372 | 0.7284× | 0.72× | ✅ |
| upload | 416,425 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 25,858 | 40,999 | 0.6307× | 0.64× | ✅ |
| client | 18,608 | 19,730 | 0.9431× | 0.93× | ✅ |
| socket | 19,850 | 21,045 | 0.9432× | 0.93× | ✅ |
| pgsql | 22,805 | 24,666 | 0.9245× | 0.92× | ✅ |
| sqlite | 23,500 | 25,268 | 0.9300× | 0.93× | ✅ |
| curl | 86,112 | 90,546 | 0.9510× | 0.95× | ✅ |
| dns | 230,044 | 232,917 | 0.9877× | 0.99× | ✅ |
| file | 108,916 | 131,297 | 0.8295× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
