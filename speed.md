# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `597bc97` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,384 | 15,210 | 0.9457× | 0.91× | ✅ |
| echo-1k | 17,377 | 18,172 | 0.9563× | 0.92× | ✅ |
| json | 22,381 | 23,178 | 0.9656× | 0.94× | ✅ |
| service-health | 16,951 | 17,791 | 0.9528× | 0.93× | ✅ |
| websocket | 6,577 | 8,191 | 0.8029× | 0.81× | ✅ |
| timer | 201,380 | 313,367 | 0.6426× | 0.67× | ✅ |
| app | 125,269 | 135,947 | 0.9215× | 0.92× | ✅ |
| http2 | 19,808 | 20,467 | 0.9678× | 0.95× | ✅ |
| table | 197,398 | 202,907 | 0.9728× | 0.98× | ✅ |
| channel | 139,252 | 155,189 | 0.8973× | 0.89× | ✅ |
| task | 31,884 | 33,268 | 0.9584× | 0.94× | ✅ |
| mysql | 49,437 | 51,076 | 0.9679× | 0.97× | ✅ |
| http-client | 33,564 | 35,823 | 0.9369× | 0.95× | ✅ |
| client | 18,623 | 19,774 | 0.9417× | 0.95× | ✅ |
| socket | 20,101 | 21,091 | 0.9531× | 0.99× | ✅ |
| gzip | 200,695 | 197,094 | 1.0183× | 1.02× | ➖ |
| websocket-deflate | 181,377 | 181,560 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,743 | 1.0682× | 1.02× | ❌ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,791 | 7,365 | 1.0579× | 1.09× | ➖ |
| connect | 20,341 | 18,707 | 1.0874× | 1.09× | ➖ |
| https | 109,165 | 108,803 | 1.0033× | 1.04× | ➖ |
| wss | 104,257 | 104,801 | 0.9948× | 1.04× | ✅ |
| atomic | 52,338 | 51,334 | 1.0196× | 1.02× | ➖ |
| process-mode | 16,522 | 16,949 | 0.9748× | 0.95× | ✅ |
| http2-concurrent | 1,460,046 | 408,816 | 3.5714× | 1.31× | ❌ |
| websocket-64k | 156,025 | 159,692 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,561 | 16,193 | 0.8375× | 0.78× | ✅ |
| upload | 1,780,612 | 1,322,430 | 1.3465× | 1.05× | ❌ |
| static | 39,303 | 41,057 | 0.9573× | 1.0× | ✅ |

Geometric mean 0.9866×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
