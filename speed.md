# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `05a7bc6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,207 | 0.9047× | 0.91× | ✅ |
| echo-1k | 16,803 | 18,162 | 0.9252× | 0.92× | ✅ |
| json | 22,086 | 23,170 | 0.9532× | 0.95× | ✅ |
| service-health | 16,344 | 17,782 | 0.9191× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 201,973 | 313,881 | 0.6435× | 0.65× | ✅ |
| app | 127,612 | 136,016 | 0.9382× | 0.93× | ✅ |
| http2 | 18,965 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,369 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,082 | 33,272 | 0.9642× | 0.96× | ✅ |
| mysql | 50,320 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,657 | 35,792 | 0.9404× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,613 | 7,987 | 0.9533× | 0.95× | ✅ |
| connect | 17,530 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,291 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,577 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,199 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,418 | 17,027 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 363,409 | 409,053 | 0.8884× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,522 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,903 | 41,063 | 0.6308× | 0.64× | ✅ |
| client | 18,624 | 19,730 | 0.9440× | 0.93× | ✅ |
| socket | 19,863 | 21,045 | 0.9438× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,128 | 90,379 | 0.9530× | 0.95× | ✅ |
| dns | 229,652 | 232,392 | 0.9882× | 0.99× | ✅ |
| file | 109,087 | 131,300 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
