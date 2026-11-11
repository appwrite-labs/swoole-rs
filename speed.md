# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f818078` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,648 | 15,210 | 0.9630× | 0.97× | ✅ |
| echo-1k | 17,833 | 18,179 | 0.9810× | 0.99× | ✅ |
| json | 22,698 | 23,167 | 0.9798× | 0.98× | ✅ |
| service-health | 17,252 | 17,782 | 0.9702× | 0.98× | ✅ |
| websocket | 6,859 | 8,188 | 0.8376× | 0.84× | ✅ |
| timer | 202,990 | 313,340 | 0.6478× | 0.65× | ✅ |
| app | 127,012 | 135,875 | 0.9348× | 0.94× | ✅ |
| http2 | 20,259 | 20,464 | 0.9900× | 0.99× | ✅ |
| table | 196,601 | 201,874 | 0.9739× | 0.98× | ✅ |
| channel | 139,955 | 154,667 | 0.9049× | 0.91× | ✅ |
| task | 32,630 | 33,274 | 0.9807× | 0.99× | ✅ |
| mysql | 50,258 | 51,058 | 0.9843× | 0.99× | ✅ |
| http-client | 34,716 | 35,830 | 0.9689× | 0.97× | ✅ |
| client | 19,167 | 19,766 | 0.9697× | 0.98× | ✅ |
| socket | 20,634 | 21,071 | 0.9793× | 0.98× | ✅ |
| gzip | 196,697 | 197,097 | 0.9980× | 1.0× | ✅ |
| websocket-deflate | 181,637 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.14× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,215 | 7,360 | 0.9804× | 1.01× | ✅ |
| connect | 19,469 | 18,707 | 1.0407× | 1.15× | ➖ |
| https | 109,910 | 108,810 | 1.0101× | 1.01× | ❌ |
| wss | 105,090 | 104,802 | 1.0028× | 1.0× | ❌ |
| atomic | 50,472 | 51,325 | 0.9834× | 1.04× | ✅ |
| process-mode | 16,930 | 16,960 | 0.9982× | 1.0× | ✅ |
| http2-concurrent | 376,191 | 409,754 | 0.9181× | 0.93× | ✅ |
| websocket-64k | 156,302 | 159,699 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,216 | 16,202 | 0.8774× | 0.87× | ✅ |
| upload | 674,358 | 1,322,466 | 0.5099× | 0.52× | ✅ |
| static | 35,066 | 41,045 | 0.8543× | 0.9× | ✅ |

Geometric mean 0.9190×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
