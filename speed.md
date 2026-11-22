# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b6bb702` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,161 | 0.9535× | 0.95× | ✅ |
| service-health | 16,341 | 17,773 | 0.9194× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,018 | 313,783 | 0.6438× | 0.65× | ✅ |
| app | 127,653 | 135,948 | 0.9390× | 0.93× | ✅ |
| http2 | 18,965 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,714 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,312 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,658 | 35,797 | 0.9402× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,800 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,345 | 409,057 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,006 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,513 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,924 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,825 | 24,662 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,135 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 229,858 | 232,892 | 0.9870× | 0.99× | ✅ |
| file | 109,096 | 131,208 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
