# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d32e071` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,380 | 15,214 | 0.9452× | 0.91× | ✅ |
| echo-1k | 17,381 | 18,175 | 0.9563× | 0.92× | ✅ |
| json | 22,377 | 23,178 | 0.9655× | 0.94× | ✅ |
| service-health | 16,948 | 17,795 | 0.9524× | 0.93× | ✅ |
| websocket | 6,573 | 8,188 | 0.8028× | 0.81× | ✅ |
| timer | 201,419 | 313,441 | 0.6426× | 0.67× | ✅ |
| app | 125,294 | 135,944 | 0.9217× | 0.92× | ✅ |
| http2 | 19,806 | 20,464 | 0.9679× | 0.95× | ✅ |
| table | 197,398 | 202,904 | 0.9729× | 0.98× | ✅ |
| channel | 139,252 | 155,189 | 0.8973× | 0.89× | ✅ |
| task | 31,883 | 33,265 | 0.9584× | 0.94× | ✅ |
| mysql | 49,437 | 51,076 | 0.9679× | 0.97× | ✅ |
| http-client | 33,566 | 35,825 | 0.9370× | 0.95× | ✅ |
| client | 18,622 | 19,775 | 0.9417× | 0.95× | ✅ |
| socket | 20,102 | 21,090 | 0.9531× | 0.99× | ✅ |
| gzip | 200,695 | 197,089 | 1.0183× | 1.02× | ➖ |
| websocket-deflate | 181,377 | 181,560 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,744 | 1.0680× | 1.02× | ❌ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,476 | 7,321 | 1.0213× | 1.09× | ➖ |
| connect | 20,345 | 18,703 | 1.0878× | 1.09× | ➖ |
| https | 109,169 | 108,814 | 1.0033× | 1.04× | ➖ |
| wss | 104,260 | 104,794 | 0.9949× | 1.04× | ✅ |
| atomic | 52,341 | 51,334 | 1.0196× | 1.02× | ➖ |
| process-mode | 16,522 | 16,948 | 0.9749× | 0.95× | ✅ |
| http2-concurrent | 1,459,541 | 408,816 | 3.5702× | 1.31× | ❌ |
| websocket-64k | 156,024 | 159,693 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,561 | 16,194 | 0.8374× | 0.78× | ✅ |
| upload | 1,780,622 | 1,322,444 | 1.3465× | 1.05× | ❌ |
| static | 39,262 | 41,028 | 0.9569× | 1.0× | ✅ |

Geometric mean 0.9855×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
