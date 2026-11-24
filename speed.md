# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,770 | 15,210 | 0.9053× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,104 | 23,161 | 0.9544× | 0.95× | ✅ |
| service-health | 16,361 | 17,773 | 0.9205× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,987 | 313,826 | 0.6436× | 0.65× | ✅ |
| app | 127,493 | 135,966 | 0.9377× | 0.93× | ✅ |
| http2 | 19,048 | 20,496 | 0.9294× | 0.92× | ✅ |
| table | 194,394 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,386 | 155,714 | 0.8823× | 0.88× | ✅ |
| task | 32,090 | 33,269 | 0.9646× | 0.96× | ✅ |
| mysql | 50,270 | 51,085 | 0.9840× | 0.97× | ✅ |
| http-client | 33,621 | 35,784 | 0.9395× | 0.93× | ✅ |
| gzip | 194,284 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,581 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,203 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,431 | 17,028 | 0.9062× | 0.89× | ✅ |
| http2-concurrent | 364,540 | 409,053 | 0.8912× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,516 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,862 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,613 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,853 | 21,045 | 0.9433× | 0.93× | ✅ |
| pgsql | 22,820 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,132 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 229,838 | 232,752 | 0.9875× | 0.99× | ✅ |
| file | 108,940 | 131,208 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
