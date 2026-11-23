# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `18f368d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,214 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,721 | 8,189 | 0.8208× | 0.81× | ✅ |
| timer | 201,998 | 314,025 | 0.6433× | 0.65× | ✅ |
| app | 127,608 | 135,946 | 0.9387× | 0.93× | ✅ |
| http2 | 18,964 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,269 | 0.9644× | 0.96× | ✅ |
| mysql | 50,312 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,625 | 35,793 | 0.9394× | 0.93× | ✅ |
| gzip | 194,285 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,163 | 181,570 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,802 | 0.9771× | 0.98× | ✅ |
| wss | 101,591 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,282 | 409,053 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,924 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,621 | 19,729 | 0.9438× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,815 | 24,663 | 0.9251× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,128 | 90,410 | 0.9526× | 0.95× | ✅ |
| dns | 230,197 | 232,559 | 0.9898× | 0.99× | ✅ |
| file | 109,088 | 131,297 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
