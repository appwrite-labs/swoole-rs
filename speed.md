# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `77448c4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,256 | 15,210 | 0.9373× | 0.97× | ✅ |
| echo-1k | 17,383 | 18,163 | 0.9570× | 0.99× | ✅ |
| json | 22,345 | 23,167 | 0.9645× | 0.98× | ✅ |
| service-health | 16,849 | 17,786 | 0.9474× | 0.97× | ✅ |
| websocket | 6,858 | 8,191 | 0.8372× | 0.84× | ✅ |
| timer | 202,622 | 313,386 | 0.6466× | 0.65× | ✅ |
| app | 126,557 | 135,928 | 0.9311× | 0.94× | ✅ |
| http2 | 20,219 | 20,467 | 0.9879× | 0.99× | ✅ |
| table | 196,193 | 201,874 | 0.9719× | 0.98× | ✅ |
| channel | 139,549 | 154,667 | 0.9023× | 0.91× | ✅ |
| task | 32,216 | 33,270 | 0.9683× | 0.99× | ✅ |
| mysql | 49,888 | 51,062 | 0.9770× | 0.99× | ✅ |
| http-client | 34,069 | 35,830 | 0.9509× | 0.97× | ✅ |
| client | 18,765 | 19,766 | 0.9493× | 0.97× | ✅ |
| socket | 20,231 | 21,071 | 0.9602× | 0.98× | ✅ |
| gzip | 196,252 | 197,098 | 0.9957× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,986 | 0.9994× | 1.04× | ✅ |
| connect | 19,088 | 18,707 | 1.0204× | 1.05× | ➖ |
| https | 109,484 | 108,810 | 1.0062× | 1.01× | ➖ |
| wss | 105,104 | 104,802 | 1.0029× | 1.0× | ❌ |
| atomic | 50,069 | 51,325 | 0.9755× | 0.99× | ✅ |
| process-mode | 16,525 | 17,024 | 0.9707× | 1.0× | ✅ |
| http2-concurrent | 375,561 | 409,073 | 0.9181× | 0.92× | ✅ |
| websocket-64k | 156,300 | 159,706 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,216 | 16,202 | 0.8774× | 0.87× | ✅ |
| upload | 673,898 | 1,322,468 | 0.5096× | 0.51× | ✅ |
| static | 35,136 | 41,045 | 0.8560× | 0.86× | ✅ |

Geometric mean 0.9121×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
