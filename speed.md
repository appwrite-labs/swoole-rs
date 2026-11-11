# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5f65c0e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,644 | 15,213 | 0.9626× | 0.97× | ✅ |
| echo-1k | 17,821 | 18,168 | 0.9809× | 0.99× | ✅ |
| json | 22,698 | 23,170 | 0.9796× | 0.98× | ✅ |
| service-health | 17,249 | 17,786 | 0.9698× | 0.98× | ✅ |
| websocket | 6,859 | 8,188 | 0.8378× | 0.84× | ✅ |
| timer | 203,178 | 313,375 | 0.6484× | 0.65× | ✅ |
| app | 126,936 | 135,997 | 0.9334× | 0.94× | ✅ |
| http2 | 20,268 | 20,467 | 0.9903× | 0.99× | ✅ |
| table | 196,601 | 201,874 | 0.9739× | 0.98× | ✅ |
| channel | 139,951 | 154,664 | 0.9049× | 0.91× | ✅ |
| task | 32,623 | 33,286 | 0.9801× | 0.99× | ✅ |
| mysql | 50,263 | 51,061 | 0.9844× | 0.99× | ✅ |
| http-client | 34,705 | 35,829 | 0.9686× | 0.97× | ✅ |
| client | 19,166 | 19,766 | 0.9697× | 0.98× | ✅ |
| socket | 20,634 | 21,070 | 0.9793× | 0.98× | ✅ |
| gzip | 196,693 | 197,097 | 0.9979× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.14× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,263 | 7,355 | 0.9875× | 1.01× | ✅ |
| connect | 19,473 | 18,703 | 1.0411× | 1.15× | ➖ |
| https | 109,913 | 108,803 | 1.0102× | 1.01× | ❌ |
| wss | 105,104 | 104,802 | 1.0029× | 1.0× | ❌ |
| atomic | 50,469 | 51,325 | 0.9833× | 1.04× | ✅ |
| process-mode | 16,931 | 16,960 | 0.9983× | 1.0× | ✅ |
| http2-concurrent | 375,950 | 409,073 | 0.9190× | 0.93× | ✅ |
| websocket-64k | 156,301 | 159,702 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,216 | 16,201 | 0.8775× | 0.87× | ✅ |
| upload | 674,366 | 1,322,447 | 0.5099× | 0.52× | ✅ |
| static | 35,066 | 41,045 | 0.8543× | 0.9× | ✅ |

Geometric mean 0.9192×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
