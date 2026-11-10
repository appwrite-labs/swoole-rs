# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a28b800` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,783 | 15,214 | 1.6290× | 0.95× | ❌ |
| echo-1k | 32,677 | 18,169 | 1.7985× | 0.96× | ❌ |
| json | 33,353 | 23,178 | 1.4390× | 0.97× | ❌ |
| service-health | 27,560 | 17,791 | 1.5491× | 0.96× | ❌ |
| websocket | 6,596 | 8,188 | 0.8056× | 0.81× | ✅ |
| timer | 211,742 | 313,460 | 0.6755× | 0.65× | ✅ |
| app | 140,861 | 135,964 | 1.0360× | 0.92× | ❌ |
| http2 | 19,635 | 20,467 | 0.9593× | 0.97× | ✅ |
| table | 207,972 | 202,907 | 1.0250× | 0.98× | ❌ |
| channel | 148,143 | 155,192 | 0.9546× | 0.89× | ✅ |
| task | 47,016 | 33,269 | 1.4132× | 0.96× | ❌ |
| mysql | 59,961 | 51,079 | 1.1739× | 0.97× | ❌ |
| http-client | 52,591 | 35,823 | 1.4681× | 0.94× | ❌ |
| client | 29,232 | 19,775 | 1.4782× | 0.95× | ❌ |
| socket | 30,710 | 21,091 | 1.4561× | 0.99× | ❌ |
| gzip | 215,862 | 197,096 | 1.0952× | 1.02× | ❌ |
| websocket-deflate | 181,394 | 181,564 | 0.9991× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,746 | 1.0675× | 1.07× | ➖ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,603 | 7,342 | 1.0356× | 1.07× | ➖ |
| connect | 33,397 | 18,707 | 1.7853× | 1.09× | ❌ |
| https | 119,554 | 108,814 | 1.0987× | 1.01× | ❌ |
| wss | 104,288 | 104,797 | 0.9951× | 1.0× | ✅ |
| atomic | 66,166 | 51,334 | 1.2889× | 1.02× | ❌ |
| process-mode | 26,913 | 17,010 | 1.5822× | 0.98× | ❌ |
| http2-concurrent | 1,457,136 | 409,546 | 3.5579× | 1.31× | ❌ |
| websocket-64k | 156,037 | 159,691 | 0.9771× | 0.62× | ✅ |
| websocket-fragmented | 13,936 | 16,194 | 0.8605× | 0.78× | ✅ |
| upload | 1,800,100 | 1,322,466 | 1.3612× | 1.05× | ❌ |
| static | 39,233 | 40,999 | 0.9569× | 0.96× | ✅ |

Geometric mean 1.1904×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
