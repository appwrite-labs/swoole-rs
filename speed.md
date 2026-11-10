# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2c32000` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,378 | 15,209 | 0.9454× | 0.91× | ✅ |
| echo-1k | 17,375 | 18,168 | 0.9564× | 0.92× | ✅ |
| json | 22,383 | 23,173 | 0.9659× | 0.94× | ✅ |
| service-health | 16,954 | 17,786 | 0.9532× | 0.93× | ✅ |
| websocket | 6,565 | 8,188 | 0.8018× | 0.81× | ✅ |
| timer | 208,679 | 313,408 | 0.6658× | 0.68× | ✅ |
| app | 125,828 | 135,947 | 0.9256× | 0.92× | ✅ |
| http2 | 19,802 | 20,488 | 0.9665× | 0.95× | ✅ |
| table | 197,912 | 201,934 | 0.9801× | 0.98× | ✅ |
| channel | 139,772 | 155,262 | 0.9002× | 0.89× | ✅ |
| task | 31,871 | 33,313 | 0.9567× | 0.94× | ✅ |
| mysql | 49,542 | 51,064 | 0.9702× | 0.97× | ✅ |
| http-client | 33,711 | 35,827 | 0.9409× | 0.97× | ✅ |
| client | 18,542 | 19,612 | 0.9454× | 0.96× | ✅ |
| socket | 19,242 | 18,381 | 1.0469× | 0.99× | ❌ |
| gzip | 200,790 | 197,090 | 1.0188× | 1.04× | ➖ |
| websocket-deflate | 181,369 | 181,572 | 0.9989× | 1.04× | ✅ |
| tcp-length | 5,054 | 4,743 | 1.0656× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.57× | ✅ |
| udp | 8,547 | 7,334 | 1.1655× | 1.09× | ❌ |
| https | 109,161 | 108,803 | 1.0033× | 1.04× | ➖ |
| wss | 104,259 | 104,802 | 0.9948× | 1.04× | ✅ |
| atomic | 52,344 | 51,334 | 1.0197× | 1.02× | ➖ |
| process-mode | 16,495 | 16,948 | 0.9732× | 0.95× | ✅ |

Geometric mean 0.9366×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
