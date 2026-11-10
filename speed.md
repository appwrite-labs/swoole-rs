# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0f2be4a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,779 | 15,214 | 1.6287× | 0.95× | ❌ |
| echo-1k | 32,675 | 18,179 | 1.7974× | 0.96× | ❌ |
| json | 33,349 | 23,178 | 1.4388× | 0.97× | ❌ |
| service-health | 27,560 | 17,795 | 1.5488× | 0.96× | ❌ |
| websocket | 6,593 | 8,191 | 0.8049× | 0.81× | ✅ |
| timer | 211,720 | 313,657 | 0.6750× | 0.67× | ✅ |
| app | 140,821 | 135,949 | 1.0358× | 0.92× | ❌ |
| http2 | 19,625 | 20,467 | 0.9589× | 0.97× | ✅ |
| table | 207,970 | 202,907 | 1.0250× | 0.98× | ❌ |
| channel | 148,147 | 155,192 | 0.9546× | 0.89× | ✅ |
| task | 47,018 | 33,292 | 1.4123× | 0.96× | ❌ |
| mysql | 59,969 | 51,076 | 1.1741× | 0.97× | ❌ |
| http-client | 52,590 | 35,825 | 1.4680× | 0.95× | ❌ |
| client | 29,231 | 19,775 | 1.4782× | 0.95× | ❌ |
| socket | 30,710 | 21,090 | 1.4561× | 0.99× | ❌ |
| gzip | 215,854 | 197,093 | 1.0952× | 1.02× | ❌ |
| websocket-deflate | 181,391 | 181,560 | 0.9991× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,746 | 1.0675× | 1.07× | ➖ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,810 | 7,340 | 1.0639× | 1.09× | ➖ |
| connect | 33,397 | 18,707 | 1.7853× | 1.09× | ❌ |
| https | 119,556 | 108,803 | 1.0988× | 1.04× | ❌ |
| wss | 104,283 | 104,794 | 0.9951× | 1.04× | ✅ |
| atomic | 66,166 | 51,334 | 1.2889× | 1.02× | ❌ |
| process-mode | 26,916 | 16,949 | 1.5881× | 0.98× | ❌ |
| http2-concurrent | 1,456,859 | 408,938 | 3.5625× | 1.31× | ❌ |
| websocket-64k | 156,050 | 159,695 | 0.9772× | 0.62× | ✅ |
| websocket-fragmented | 13,935 | 16,195 | 0.8605× | 0.78× | ✅ |
| upload | 1,800,103 | 1,322,467 | 1.3612× | 1.05× | ❌ |
| static | 39,274 | 41,028 | 0.9572× | 1.0× | ✅ |

Geometric mean 1.1915×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
