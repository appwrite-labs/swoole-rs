# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e4da832` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,384 | 15,210 | 0.9457× | 0.91× | ✅ |
| echo-1k | 17,377 | 18,167 | 0.9565× | 0.92× | ✅ |
| json | 22,381 | 23,178 | 0.9656× | 0.94× | ✅ |
| service-health | 16,948 | 17,795 | 0.9524× | 0.93× | ✅ |
| websocket | 6,573 | 8,191 | 0.8024× | 0.81× | ✅ |
| timer | 201,414 | 313,366 | 0.6427× | 0.67× | ✅ |
| app | 125,296 | 135,874 | 0.9222× | 0.92× | ✅ |
| http2 | 19,802 | 20,467 | 0.9675× | 0.95× | ✅ |
| table | 197,398 | 202,907 | 0.9728× | 0.98× | ✅ |
| channel | 139,248 | 155,192 | 0.8973× | 0.89× | ✅ |
| task | 31,879 | 33,273 | 0.9581× | 0.94× | ✅ |
| mysql | 49,434 | 51,079 | 0.9678× | 0.97× | ✅ |
| http-client | 33,562 | 35,824 | 0.9369× | 0.95× | ✅ |
| client | 18,622 | 19,775 | 0.9417× | 0.95× | ✅ |
| socket | 20,101 | 21,086 | 0.9533× | 0.99× | ✅ |
| gzip | 200,706 | 197,096 | 1.0183× | 1.02× | ➖ |
| websocket-deflate | 181,380 | 181,564 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,746 | 1.0675× | 1.02× | ❌ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,766 | 7,339 | 1.0582× | 1.09× | ➖ |
| connect | 20,341 | 18,707 | 1.0874× | 1.09× | ➖ |
| https | 109,165 | 108,810 | 1.0033× | 1.04× | ➖ |
| wss | 104,271 | 104,801 | 0.9950× | 1.04× | ✅ |
| atomic | 52,341 | 51,334 | 1.0196× | 1.02× | ➖ |
| process-mode | 16,522 | 17,010 | 0.9713× | 0.95× | ✅ |
| http2-concurrent | 1,459,633 | 408,817 | 3.5704× | 1.31× | ❌ |
| websocket-64k | 156,024 | 159,691 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,561 | 16,194 | 0.8374× | 0.78× | ✅ |
| upload | 1,780,653 | 1,322,464 | 1.3465× | 1.05× | ❌ |
| static | 39,233 | 40,999 | 0.9569× | 1.0× | ✅ |

Geometric mean 0.9865×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
