# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be7677e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,164 | 0.9533× | 0.95× | ✅ |
| service-health | 16,341 | 17,777 | 0.9192× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,947 | 313,813 | 0.6435× | 0.65× | ✅ |
| app | 127,631 | 135,960 | 0.9387× | 0.93× | ✅ |
| http2 | 18,964 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,366 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,084 | 33,269 | 0.9644× | 0.96× | ✅ |
| mysql | 50,305 | 51,085 | 0.9847× | 0.97× | ✅ |
| http-client | 33,660 | 35,796 | 0.9403× | 0.93× | ✅ |
| gzip | 194,285 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,163 | 181,573 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,416 | 17,024 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,205 | 409,053 | 0.8904× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,527 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,941 | 41,092 | 0.6313× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,857 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,134 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 230,034 | 233,138 | 0.9867× | 0.99× | ✅ |
| file | 109,096 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
