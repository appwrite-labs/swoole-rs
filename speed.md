# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4f4caae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 25,350 | 15,213 | 1.6663× | 0.95× | ❌ |
| echo-1k | 32,572 | 18,177 | 1.7920× | 0.96× | ❌ |
| json | 33,935 | 23,178 | 1.4641× | 0.97× | ❌ |
| service-health | 28,131 | 17,795 | 1.5809× | 0.96× | ❌ |
| websocket | 6,593 | 8,188 | 0.8053× | 0.81× | ✅ |
| timer | 212,337 | 313,496 | 0.6773× | 0.67× | ✅ |
| app | 140,893 | 135,818 | 1.0374× | 0.92× | ❌ |
| http2 | 19,799 | 20,464 | 0.9675× | 0.97× | ✅ |
| table | 208,541 | 202,907 | 1.0278× | 0.98× | ❌ |
| channel | 150,468 | 155,192 | 0.9696× | 0.89× | ✅ |
| task | 46,913 | 33,265 | 1.4103× | 0.96× | ❌ |
| mysql | 60,578 | 51,076 | 1.1860× | 0.97× | ❌ |
| http-client | 53,008 | 35,823 | 1.4797× | 0.95× | ❌ |
| client | 29,803 | 19,776 | 1.5070× | 0.95× | ❌ |
| socket | 31,281 | 21,090 | 1.4832× | 0.99× | ❌ |
| gzip | 215,750 | 197,090 | 1.0947× | 1.02× | ❌ |
| websocket-deflate | 181,391 | 181,560 | 0.9991× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,746 | 1.0675× | 1.07× | ➖ |
| tcp-eof | 5,604 | 10,212 | 0.5488× | 0.55× | ✅ |
| udp | 7,711 | 7,362 | 1.0475× | 1.09× | ➖ |
| connect | 33,895 | 18,707 | 1.8119× | 1.09× | ❌ |
| https | 120,122 | 108,810 | 1.1040× | 1.04× | ❌ |
| wss | 104,292 | 104,801 | 0.9951× | 1.04× | ✅ |
| atomic | 63,521 | 51,334 | 1.2374× | 1.02× | ❌ |
| process-mode | 27,490 | 16,952 | 1.6216× | 0.98× | ❌ |
| http2-concurrent | 1,459,424 | 408,818 | 3.5699× | 1.31× | ❌ |
| websocket-64k | 156,047 | 159,695 | 0.9772× | 0.62× | ✅ |
| websocket-fragmented | 13,935 | 16,193 | 0.8606× | 0.78× | ✅ |
| upload | 1,799,916 | 1,322,489 | 1.3610× | 1.05× | ❌ |
| static | 39,283 | 41,028 | 0.9575× | 1.0× | ✅ |

Geometric mean 1.1967×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
