# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d88a54a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,905 | 15,213 | 1.6371× | 0.95× | ❌ |
| echo-1k | 32,113 | 18,175 | 1.7668× | 0.96× | ❌ |
| json | 33,479 | 23,178 | 1.4444× | 0.97× | ❌ |
| service-health | 27,683 | 17,795 | 1.5557× | 0.96× | ❌ |
| websocket | 6,593 | 8,188 | 0.8052× | 0.81× | ✅ |
| timer | 211,869 | 313,408 | 0.6760× | 0.65× | ✅ |
| app | 140,286 | 135,922 | 1.0321× | 0.92× | ❌ |
| http2 | 19,610 | 20,468 | 0.9581× | 0.97× | ✅ |
| table | 208,096 | 202,907 | 1.0256× | 0.98× | ❌ |
| channel | 148,273 | 155,192 | 0.9554× | 0.89× | ✅ |
| task | 46,453 | 33,268 | 1.3964× | 0.96× | ❌ |
| mysql | 60,088 | 51,073 | 1.1765× | 0.97× | ❌ |
| http-client | 52,542 | 35,822 | 1.4668× | 0.94× | ❌ |
| client | 29,357 | 19,775 | 1.4845× | 0.95× | ❌ |
| socket | 30,837 | 21,090 | 1.4621× | 0.99× | ❌ |
| gzip | 215,292 | 197,093 | 1.0923× | 1.02× | ❌ |
| websocket-deflate | 181,391 | 181,560 | 0.9991× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,743 | 1.0682× | 1.07× | ➖ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,329 | 7,337 | 0.9989× | 1.07× | ✅ |
| connect | 33,517 | 18,707 | 1.7917× | 1.09× | ❌ |
| https | 119,682 | 108,807 | 1.0999× | 1.01× | ❌ |
| wss | 104,291 | 104,794 | 0.9952× | 1.0× | ✅ |
| atomic | 66,292 | 51,334 | 1.2914× | 1.02× | ❌ |
| process-mode | 27,034 | 17,014 | 1.5890× | 0.98× | ❌ |
| http2-concurrent | 1,456,589 | 409,185 | 3.5597× | 1.31× | ❌ |
| websocket-64k | 156,040 | 159,691 | 0.9771× | 0.62× | ✅ |
| websocket-fragmented | 13,936 | 16,194 | 0.8606× | 0.78× | ✅ |
| upload | 1,799,353 | 1,322,439 | 1.3606× | 1.05× | ❌ |
| static | 39,233 | 40,999 | 0.9569× | 0.96× | ✅ |

Geometric mean 1.1889×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
