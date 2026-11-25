# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,213 | 0.9049× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,103 | 23,164 | 0.9542× | 0.95× | ✅ |
| service-health | 16,361 | 17,777 | 0.9204× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 202,001 | 313,773 | 0.6438× | 0.65× | ✅ |
| app | 127,495 | 135,955 | 0.9378× | 0.93× | ✅ |
| http2 | 19,048 | 20,496 | 0.9294× | 0.92× | ✅ |
| table | 194,394 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,382 | 155,714 | 0.8823× | 0.88× | ✅ |
| task | 32,086 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,275 | 51,088 | 0.9841× | 0.97× | ✅ |
| http-client | 33,620 | 35,784 | 0.9395× | 0.93× | ✅ |
| gzip | 194,284 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,806 | 0.9770× | 0.98× | ✅ |
| wss | 101,582 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,431 | 17,024 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 364,296 | 409,053 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 416,506 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,943 | 41,092 | 0.6313× | 0.64× | ✅ |
| client | 18,613 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,820 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,137 | 90,418 | 0.9527× | 0.95× | ✅ |
| dns | 230,343 | 232,834 | 0.9893× | 0.99× | ✅ |
| file | 108,940 | 131,297 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
