# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d7d26b1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,260 | 15,210 | 0.9375× | 0.97× | ✅ |
| echo-1k | 17,383 | 18,162 | 0.9571× | 0.99× | ✅ |
| json | 22,345 | 23,170 | 0.9644× | 0.98× | ✅ |
| service-health | 16,849 | 17,782 | 0.9475× | 0.97× | ✅ |
| websocket | 6,855 | 8,191 | 0.8369× | 0.84× | ✅ |
| timer | 202,591 | 313,431 | 0.6464× | 0.65× | ✅ |
| app | 126,593 | 135,970 | 0.9310× | 0.94× | ✅ |
| http2 | 20,159 | 20,468 | 0.9849× | 0.99× | ✅ |
| table | 196,193 | 201,874 | 0.9719× | 0.98× | ✅ |
| channel | 139,549 | 154,667 | 0.9023× | 0.91× | ✅ |
| task | 32,217 | 33,274 | 0.9683× | 0.99× | ✅ |
| mysql | 49,888 | 51,061 | 0.9770× | 0.99× | ✅ |
| http-client | 34,073 | 35,826 | 0.9511× | 0.97× | ✅ |
| client | 18,765 | 19,766 | 0.9493× | 0.97× | ✅ |
| socket | 20,227 | 21,067 | 0.9601× | 0.98× | ✅ |
| gzip | 196,204 | 197,098 | 0.9955× | 1.0× | ✅ |
| websocket-deflate | 181,636 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,763 | 1.1267× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,986 | 0.9995× | 1.04× | ✅ |
| connect | 19,088 | 18,707 | 1.0204× | 1.05× | ➖ |
| https | 109,500 | 108,807 | 1.0064× | 1.01× | ➖ |
| wss | 105,104 | 104,802 | 1.0029× | 1.0× | ❌ |
| atomic | 50,069 | 51,325 | 0.9755× | 0.99× | ✅ |
| process-mode | 16,525 | 16,956 | 0.9746× | 1.0× | ✅ |
| http2-concurrent | 374,531 | 409,069 | 0.9156× | 0.92× | ✅ |
| websocket-64k | 156,306 | 159,699 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 14,216 | 16,202 | 0.8774× | 0.87× | ✅ |
| upload | 673,883 | 1,322,453 | 0.5096× | 0.51× | ✅ |
| static | 35,165 | 41,074 | 0.8561× | 0.86× | ✅ |

Geometric mean 0.9120×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
