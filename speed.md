# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fa6e7bd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,372 | 15,214 | 0.9447× | 0.91× | ✅ |
| echo-1k | 17,359 | 18,171 | 0.9553× | 0.92× | ✅ |
| json | 22,365 | 23,178 | 0.9649× | 0.94× | ✅ |
| service-health | 16,939 | 17,795 | 0.9519× | 0.93× | ✅ |
| websocket | 6,561 | 8,191 | 0.8010× | 0.81× | ✅ |
| timer | 201,432 | 313,503 | 0.6425× | 0.67× | ✅ |
| app | 125,525 | 135,918 | 0.9235× | 0.92× | ✅ |
| http2 | 19,783 | 20,464 | 0.9667× | 0.95× | ✅ |
| table | 197,385 | 202,907 | 0.9728× | 0.98× | ✅ |
| channel | 139,240 | 155,192 | 0.8972× | 0.89× | ✅ |
| task | 31,870 | 33,270 | 0.9579× | 0.94× | ✅ |
| mysql | 49,419 | 51,079 | 0.9675× | 0.97× | ✅ |
| http-client | 33,550 | 35,825 | 0.9365× | 0.95× | ✅ |
| client | 18,325 | 18,097 | 1.0126× | 0.95× | ❌ |
| socket | 17,744 | 19,066 | 0.9307× | 0.99× | ✅ |
| gzip | 200,689 | 197,094 | 1.0182× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,560 | 0.9989× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,744 | 1.0654× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 8,339 | 7,319 | 1.1394× | 1.09× | ❌ |
| connect | 20,321 | 18,707 | 1.0863× | 1.04× | ❌ |
| https | 109,160 | 108,814 | 1.0032× | 1.04× | ➖ |
| wss | 104,256 | 104,797 | 0.9948× | 1.04× | ✅ |
| atomic | 52,329 | 51,334 | 1.0194× | 1.02× | ➖ |
| process-mode | 16,526 | 16,952 | 0.9749× | 0.95× | ✅ |
| http2-concurrent | 1,459,126 | 408,817 | 3.5691× | 1.31× | ❌ |
| websocket-64k | 156,012 | 159,695 | 0.9769× | 0.62× | ✅ |
| websocket-fragmented | 13,549 | 16,194 | 0.8367× | 0.78× | ✅ |
| upload | 1,780,589 | 1,322,429 | 1.3465× | 1.05× | ❌ |
| static | 39,241 | 40,999 | 0.9571× | 1.0× | ✅ |

Geometric mean 0.9902×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
