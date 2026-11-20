# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,105 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,786 | 0.9199× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,028 | 314,017 | 0.6434× | 0.65× | ✅ |
| app | 127,663 | 135,899 | 0.9394× | 0.93× | ✅ |
| http2 | 18,960 | 20,496 | 0.9251× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,093 | 33,265 | 0.9648× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,682 | 35,791 | 0.9411× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,331 | 108,802 | 0.9773× | 0.98× | ✅ |
| wss | 101,591 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 363,577 | 409,057 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,926 | 16,368 | 0.7286× | 0.72× | ✅ |
| upload | 416,529 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9448× | 0.93× | ✅ |
| socket | 19,881 | 21,045 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,831 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,136 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 229,817 | 232,737 | 0.9875× | 0.99× | ✅ |
| file | 109,101 | 131,293 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
