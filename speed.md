# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f7ba7e2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,478 | 15,213 | 0.9517× | 0.94× | ✅ |
| echo-1k | 17,605 | 18,165 | 0.9691× | 0.97× | ✅ |
| json | 22,569 | 23,170 | 0.9741× | 0.97× | ✅ |
| service-health | 17,064 | 17,786 | 0.9594× | 0.95× | ✅ |
| websocket | 6,908 | 8,188 | 0.8437× | 0.84× | ✅ |
| timer | 202,518 | 313,571 | 0.6458× | 0.65× | ✅ |
| app | 126,755 | 135,850 | 0.9331× | 0.94× | ✅ |
| http2 | 20,463 | 20,468 | 0.9998× | 0.99× | ✅ |
| table | 196,350 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,767 | 154,667 | 0.9037× | 0.91× | ✅ |
| task | 32,437 | 33,261 | 0.9752× | 0.97× | ✅ |
| mysql | 50,107 | 51,044 | 0.9816× | 0.98× | ✅ |
| http-client | 34,314 | 35,827 | 0.9578× | 0.96× | ✅ |
| client | 18,982 | 19,765 | 0.9604× | 0.95× | ✅ |
| socket | 20,441 | 21,070 | 0.9701× | 0.97× | ✅ |
| gzip | 196,476 | 197,083 | 0.9969× | 1.0× | ✅ |
| websocket-deflate | 181,690 | 181,572 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0038× | 1.0× | ❌ |
| connect | 18,927 | 18,707 | 1.0117× | 1.03× | ➖ |
| https | 109,684 | 108,803 | 1.0081× | 1.01× | ➖ |
| wss | 105,073 | 104,805 | 1.0026× | 1.0× | ❌ |
| atomic | 50,287 | 51,325 | 0.9798× | 0.98× | ✅ |
| process-mode | 16,771 | 16,959 | 0.9889× | 0.98× | ✅ |
| http2-concurrent | 379,290 | 409,072 | 0.9272× | 0.92× | ✅ |
| websocket-64k | 156,360 | 159,702 | 0.9791× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,335 | 0.7885× | 0.79× | ✅ |
| upload | 675,628 | 1,322,382 | 0.5109× | 0.51× | ✅ |
| static | 34,784 | 41,045 | 0.8475× | 0.86× | ✅ |
| pgsql | 23,780 | 24,595 | 0.9668× | 0.97× | ✅ |
| sqlite | 24,076 | 25,232 | 0.9542× | 0.96× | ✅ |
| curl | 86,476 | 90,505 | 0.9555× | 0.96× | ✅ |
| dns | 231,773 | 232,462 | 0.9970× | 1.0× | ✅ |

Geometric mean 0.9201×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
