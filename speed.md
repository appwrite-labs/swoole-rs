# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `401f39a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,406 | 15,214 | 0.9469× | 0.94× | ✅ |
| echo-1k | 17,525 | 18,182 | 0.9639× | 0.96× | ✅ |
| json | 22,489 | 23,167 | 0.9707× | 0.97× | ✅ |
| service-health | 16,994 | 17,786 | 0.9555× | 0.95× | ✅ |
| websocket | 6,858 | 8,188 | 0.8376× | 0.84× | ✅ |
| timer | 202,777 | 313,513 | 0.6468× | 0.65× | ✅ |
| app | 126,686 | 135,883 | 0.9323× | 0.94× | ✅ |
| http2 | 20,322 | 20,464 | 0.9930× | 0.99× | ✅ |
| table | 196,342 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,694 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,372 | 33,268 | 0.9731× | 0.97× | ✅ |
| mysql | 50,035 | 51,064 | 0.9798× | 0.98× | ✅ |
| http-client | 34,216 | 35,827 | 0.9550× | 0.96× | ✅ |
| client | 18,909 | 19,766 | 0.9567× | 0.95× | ✅ |
| socket | 20,372 | 21,070 | 0.9668× | 0.97× | ✅ |
| gzip | 196,406 | 197,093 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,237 | 18,707 | 1.0284× | 1.03× | ➖ |
| https | 109,599 | 108,810 | 1.0073× | 1.01× | ➖ |
| wss | 105,051 | 104,808 | 1.0023× | 1.0× | ❌ |
| atomic | 50,214 | 51,321 | 0.9784× | 0.98× | ✅ |
| process-mode | 16,667 | 16,959 | 0.9828× | 0.98× | ✅ |
| http2-concurrent | 377,303 | 409,069 | 0.9223× | 0.92× | ✅ |
| websocket-64k | 156,295 | 159,700 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,010 | 16,357 | 0.8565× | 0.87× | ✅ |
| upload | 674,148 | 1,322,468 | 0.5098× | 0.51× | ✅ |
| static | 35,170 | 41,045 | 0.8569× | 0.86× | ✅ |
| pgsql | 23,695 | 24,595 | 0.9634× | 1.03× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 1.02× | ✅ |
| curl | 86,345 | 90,511 | 0.9540× | 1.01× | ✅ |
| dns | 231,394 | 232,878 | 0.9936× | 1.05× | ✅ |

Geometric mean 0.9202×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
