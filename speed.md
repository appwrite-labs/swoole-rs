# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cabb2cb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,213 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,087 | 23,174 | 0.9531× | 0.95× | ✅ |
| service-health | 16,344 | 17,782 | 0.9191× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,991 | 314,103 | 0.6431× | 0.65× | ✅ |
| app | 127,600 | 136,013 | 0.9381× | 0.93× | ✅ |
| http2 | 19,034 | 20,497 | 0.9287× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,365 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,327 | 51,088 | 0.9851× | 0.97× | ✅ |
| http-client | 33,660 | 35,784 | 0.9406× | 0.93× | ✅ |
| gzip | 194,288 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,764 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,432 | 17,024 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 364,424 | 409,057 | 0.8909× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,520 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,891 | 41,028 | 0.6310× | 0.64× | ✅ |
| client | 18,632 | 19,730 | 0.9444× | 0.93× | ✅ |
| socket | 19,871 | 21,042 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,823 | 24,662 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,126 | 90,422 | 0.9525× | 0.95× | ✅ |
| dns | 230,089 | 232,936 | 0.9878× | 0.99× | ✅ |
| file | 108,940 | 131,211 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
