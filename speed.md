# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1aa5bed` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,210 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,086 | 23,170 | 0.9532× | 0.95× | ✅ |
| service-health | 16,344 | 17,782 | 0.9191× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,998 | 313,710 | 0.6439× | 0.65× | ✅ |
| app | 127,499 | 135,943 | 0.9379× | 0.93× | ✅ |
| http2 | 19,035 | 20,496 | 0.9287× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,369 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,338 | 51,085 | 0.9854× | 0.97× | ✅ |
| http-client | 33,647 | 35,797 | 0.9399× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9539× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,435 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 364,364 | 409,053 | 0.8908× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 416,510 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,856 | 41,005 | 0.6305× | 0.64× | ✅ |
| client | 18,632 | 19,730 | 0.9443× | 0.93× | ✅ |
| socket | 19,871 | 21,045 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,820 | 24,662 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,133 | 90,550 | 0.9512× | 0.95× | ✅ |
| dns | 230,029 | 232,711 | 0.9885× | 0.99× | ✅ |
| file | 108,940 | 131,207 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
