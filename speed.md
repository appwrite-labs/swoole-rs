# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `dc6cfc8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,409 | 15,210 | 0.9474× | 0.94× | ✅ |
| echo-1k | 17,529 | 18,162 | 0.9651× | 0.96× | ✅ |
| json | 22,489 | 23,167 | 0.9707× | 0.97× | ✅ |
| service-health | 16,994 | 17,782 | 0.9557× | 0.95× | ✅ |
| websocket | 6,859 | 8,188 | 0.8377× | 0.84× | ✅ |
| timer | 202,821 | 313,576 | 0.6468× | 0.65× | ✅ |
| app | 126,700 | 135,906 | 0.9323× | 0.94× | ✅ |
| http2 | 20,322 | 20,464 | 0.9931× | 0.99× | ✅ |
| table | 196,342 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,694 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,367 | 33,273 | 0.9728× | 0.97× | ✅ |
| mysql | 50,033 | 51,063 | 0.9798× | 0.98× | ✅ |
| http-client | 34,214 | 35,825 | 0.9550× | 0.96× | ✅ |
| client | 18,910 | 19,766 | 0.9567× | 0.95× | ✅ |
| socket | 20,372 | 21,071 | 0.9668× | 0.97× | ✅ |
| gzip | 196,408 | 197,088 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,765 | 1.1261× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,241 | 18,703 | 1.0288× | 1.03× | ➖ |
| https | 109,603 | 108,813 | 1.0073× | 1.01× | ➖ |
| wss | 105,061 | 104,802 | 1.0025× | 1.0× | ❌ |
| atomic | 50,211 | 51,325 | 0.9783× | 0.98× | ✅ |
| process-mode | 16,670 | 17,027 | 0.9790× | 0.98× | ✅ |
| http2-concurrent | 377,377 | 409,069 | 0.9225× | 0.92× | ✅ |
| websocket-64k | 156,305 | 159,701 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,009 | 16,357 | 0.8564× | 0.86× | ✅ |
| upload | 674,134 | 1,322,472 | 0.5098× | 0.51× | ✅ |
| static | 35,145 | 41,045 | 0.8562× | 0.86× | ✅ |
| pgsql | 23,705 | 24,595 | 0.9638× | 0.97× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 0.96× | ✅ |
| curl | 86,350 | 90,385 | 0.9554× | 0.96× | ✅ |
| dns | 231,576 | 233,195 | 0.9931× | 1.0× | ✅ |

Geometric mean 0.9202×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
