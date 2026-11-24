# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,770 | 15,210 | 0.9053× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,103 | 23,164 | 0.9542× | 0.95× | ✅ |
| service-health | 16,361 | 17,773 | 0.9205× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 202,014 | 313,806 | 0.6438× | 0.65× | ✅ |
| app | 127,520 | 135,900 | 0.9383× | 0.93× | ✅ |
| http2 | 19,045 | 20,500 | 0.9290× | 0.92× | ✅ |
| table | 194,390 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,382 | 155,717 | 0.8823× | 0.88× | ✅ |
| task | 32,086 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,292 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,599 | 35,792 | 0.9387× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9539× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,432 | 17,024 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 364,344 | 409,053 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,369 | 0.7285× | 0.72× | ✅ |
| upload | 416,510 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,613 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,853 | 21,045 | 0.9433× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,133 | 90,549 | 0.9512× | 0.95× | ✅ |
| dns | 229,854 | 232,822 | 0.9873× | 0.99× | ✅ |
| file | 108,936 | 130,922 | 0.8321× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
