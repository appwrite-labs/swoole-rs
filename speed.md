# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4582075` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,751 | 15,210 | 0.9040× | 0.91× | ✅ |
| echo-1k | 16,784 | 18,162 | 0.9241× | 0.92× | ✅ |
| json | 22,075 | 23,170 | 0.9527× | 0.95× | ✅ |
| service-health | 16,332 | 17,782 | 0.9184× | 0.92× | ✅ |
| websocket | 6,724 | 8,192 | 0.8208× | 0.81× | ✅ |
| timer | 201,933 | 313,788 | 0.6435× | 0.65× | ✅ |
| app | 127,591 | 135,923 | 0.9387× | 0.93× | ✅ |
| http2 | 19,075 | 20,496 | 0.9307× | 0.92× | ✅ |
| table | 194,301 | 201,889 | 0.9624× | 0.96× | ✅ |
| channel | 137,357 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 32,074 | 33,267 | 0.9641× | 0.96× | ✅ |
| mysql | 50,318 | 51,088 | 0.9849× | 0.97× | ✅ |
| http-client | 33,710 | 35,795 | 0.9417× | 0.93× | ✅ |
| gzip | 194,278 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,300 | 181,570 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,618 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,523 | 18,730 | 0.9355× | 0.93× | ✅ |
| https | 106,293 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,580 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,187 | 51,285 | 0.9786× | 0.97× | ✅ |
| process-mode | 15,413 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 366,216 | 409,057 | 0.8953× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,929 | 16,368 | 0.7288× | 0.72× | ✅ |
| upload | 416,432 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 25,864 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,622 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,856 | 21,046 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,810 | 24,663 | 0.9249× | 0.92× | ✅ |
| sqlite | 23,509 | 25,253 | 0.9309× | 0.93× | ✅ |
| curl | 86,118 | 90,422 | 0.9524× | 0.95× | ✅ |
| dns | 230,054 | 232,769 | 0.9883× | 0.99× | ✅ |
| file | 108,925 | 130,922 | 0.8320× | 0.83× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
