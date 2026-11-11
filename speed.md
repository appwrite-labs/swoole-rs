# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `510d091` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,393 | 15,214 | 0.9461× | 0.94× | ✅ |
| echo-1k | 17,643 | 18,165 | 0.9713× | 0.97× | ✅ |
| json | 22,482 | 23,170 | 0.9703× | 0.97× | ✅ |
| service-health | 16,983 | 17,786 | 0.9549× | 0.95× | ✅ |
| websocket | 6,864 | 8,188 | 0.8384× | 0.84× | ✅ |
| timer | 202,781 | 313,819 | 0.6462× | 0.65× | ✅ |
| app | 126,716 | 135,938 | 0.9322× | 0.94× | ✅ |
| http2 | 20,325 | 20,468 | 0.9930× | 0.99× | ✅ |
| table | 196,334 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,686 | 154,667 | 0.9031× | 0.91× | ✅ |
| task | 32,473 | 33,263 | 0.9762× | 0.97× | ✅ |
| mysql | 50,018 | 51,052 | 0.9797× | 0.98× | ✅ |
| http-client | 34,206 | 35,829 | 0.9547× | 0.96× | ✅ |
| client | 18,901 | 19,766 | 0.9563× | 0.95× | ✅ |
| socket | 20,364 | 21,070 | 0.9665× | 0.97× | ✅ |
| gzip | 196,521 | 197,079 | 0.9972× | 1.0× | ✅ |
| websocket-deflate | 181,642 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,986 | 0.9995× | 1.0× | ✅ |
| connect | 19,253 | 18,707 | 1.0292× | 1.03× | ➖ |
| https | 109,629 | 108,813 | 1.0075× | 1.01× | ➖ |
| wss | 105,056 | 104,805 | 1.0024× | 1.0× | ❌ |
| atomic | 50,206 | 51,325 | 0.9782× | 0.98× | ✅ |
| process-mode | 16,652 | 16,956 | 0.9821× | 0.98× | ✅ |
| http2-concurrent | 378,068 | 409,072 | 0.9242× | 0.92× | ✅ |
| websocket-64k | 156,317 | 159,706 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 14,019 | 16,358 | 0.8570× | 0.87× | ✅ |
| upload | 674,261 | 1,322,380 | 0.5099× | 0.51× | ✅ |
| static | 35,157 | 41,045 | 0.8565× | 0.86× | ✅ |
| pgsql | 23,696 | 24,595 | 0.9635× | 0.97× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 0.96× | ✅ |
| curl | 86,398 | 90,509 | 0.9546× | 0.96× | ✅ |
| dns | 231,783 | 232,832 | 0.9955× | 1.0× | ✅ |

Geometric mean 0.9206×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
