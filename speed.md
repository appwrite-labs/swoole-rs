# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d61d414` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,214 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,093 | 23,170 | 0.9535× | 0.95× | ✅ |
| service-health | 16,351 | 17,782 | 0.9195× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,994 | 313,776 | 0.6438× | 0.65× | ✅ |
| app | 127,517 | 135,982 | 0.9378× | 0.93× | ✅ |
| http2 | 19,034 | 20,496 | 0.9287× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,376 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,086 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,296 | 51,088 | 0.9845× | 0.97× | ✅ |
| http-client | 33,599 | 35,793 | 0.9387× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,579 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,432 | 17,021 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,403 | 409,057 | 0.8908× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,372 | 0.7286× | 0.72× | ✅ |
| upload | 416,520 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,862 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,610 | 19,730 | 0.9432× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,823 | 24,662 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,132 | 90,528 | 0.9514× | 0.95× | ✅ |
| dns | 230,095 | 233,145 | 0.9869× | 0.99× | ✅ |
| file | 108,940 | 131,297 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
