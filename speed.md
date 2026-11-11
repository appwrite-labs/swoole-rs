# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a8d8c0b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,648 | 15,210 | 0.9631× | 0.97× | ✅ |
| echo-1k | 17,826 | 18,177 | 0.9807× | 0.99× | ✅ |
| json | 22,698 | 23,167 | 0.9798× | 0.98× | ✅ |
| service-health | 17,252 | 17,782 | 0.9702× | 0.98× | ✅ |
| websocket | 6,859 | 8,188 | 0.8378× | 0.84× | ✅ |
| timer | 203,045 | 313,264 | 0.6482× | 0.65× | ✅ |
| app | 126,992 | 135,976 | 0.9339× | 0.94× | ✅ |
| http2 | 20,261 | 20,464 | 0.9901× | 0.99× | ✅ |
| table | 196,601 | 201,874 | 0.9739× | 0.98× | ✅ |
| channel | 139,955 | 154,664 | 0.9049× | 0.91× | ✅ |
| task | 32,629 | 33,268 | 0.9808× | 0.99× | ✅ |
| mysql | 50,263 | 51,061 | 0.9844× | 0.99× | ✅ |
| http-client | 34,717 | 35,829 | 0.9690× | 0.97× | ✅ |
| client | 19,168 | 19,766 | 0.9697× | 0.98× | ✅ |
| socket | 20,638 | 21,067 | 0.9796× | 0.98× | ✅ |
| gzip | 196,689 | 197,100 | 0.9979× | 1.0× | ✅ |
| websocket-deflate | 181,637 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.14× | ➖ |
| tcp-eof | 5,860 | 10,228 | 0.5730× | 0.58× | ✅ |
| udp | 7,219 | 7,343 | 0.9831× | 1.01× | ✅ |
| connect | 19,473 | 18,703 | 1.0411× | 1.15× | ➖ |
| https | 109,897 | 108,814 | 1.0100× | 1.01× | ➖ |
| wss | 105,087 | 104,805 | 1.0027× | 1.0× | ❌ |
| atomic | 50,472 | 51,325 | 0.9834× | 1.04× | ✅ |
| process-mode | 16,927 | 16,960 | 0.9981× | 1.0× | ✅ |
| http2-concurrent | 376,432 | 409,072 | 0.9202× | 0.93× | ✅ |
| websocket-64k | 156,297 | 159,702 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,215 | 16,202 | 0.8774× | 0.87× | ✅ |
| upload | 674,365 | 1,322,457 | 0.5099× | 0.52× | ✅ |
| static | 35,066 | 41,045 | 0.8543× | 0.9× | ✅ |

Geometric mean 0.9191×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
