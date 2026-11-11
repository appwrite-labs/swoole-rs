# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4281ef3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,648 | 15,214 | 0.9628× | 0.97× | ✅ |
| echo-1k | 17,828 | 18,167 | 0.9813× | 0.99× | ✅ |
| json | 22,698 | 23,158 | 0.9802× | 0.98× | ✅ |
| service-health | 17,252 | 17,773 | 0.9707× | 0.98× | ✅ |
| websocket | 6,859 | 8,188 | 0.8377× | 0.84× | ✅ |
| timer | 203,015 | 313,398 | 0.6478× | 0.65× | ✅ |
| app | 126,994 | 135,934 | 0.9342× | 0.94× | ✅ |
| http2 | 20,265 | 20,464 | 0.9903× | 0.99× | ✅ |
| table | 196,601 | 201,865 | 0.9739× | 0.98× | ✅ |
| channel | 139,435 | 154,658 | 0.9016× | 0.91× | ✅ |
| task | 32,619 | 33,274 | 0.9803× | 0.99× | ✅ |
| mysql | 50,266 | 51,055 | 0.9846× | 0.99× | ✅ |
| http-client | 34,711 | 35,819 | 0.9691× | 0.97× | ✅ |
| client | 19,167 | 19,756 | 0.9702× | 0.98× | ✅ |
| socket | 20,629 | 21,062 | 0.9794× | 0.98× | ✅ |
| gzip | 196,698 | 197,098 | 0.9980× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,580 | 1.0003× | 1.0× | ❌ |
| tcp-length | 5,346 | 4,743 | 1.1272× | 1.14× | ➖ |
| tcp-eof | 5,840 | 10,212 | 0.5719× | 0.58× | ✅ |
| udp | 7,310 | 7,335 | 0.9966× | 1.01× | ✅ |
| connect | 19,473 | 18,707 | 1.0409× | 1.15× | ➖ |
| https | 109,880 | 108,810 | 1.0098× | 1.01× | ➖ |
| wss | 105,105 | 104,794 | 1.0030× | 1.0× | ❌ |
| atomic | 50,472 | 51,316 | 0.9836× | 1.04× | ✅ |
| process-mode | 16,898 | 16,918 | 0.9988× | 1.0× | ✅ |
| http2-concurrent | 376,092 | 408,877 | 0.9198× | 0.93× | ✅ |
| websocket-64k | 156,304 | 159,693 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 14,214 | 16,193 | 0.8778× | 0.87× | ✅ |
| upload | 674,356 | 1,322,407 | 0.5099× | 0.52× | ✅ |
| static | 35,107 | 41,074 | 0.8547× | 0.9× | ✅ |

Geometric mean 0.9195×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
