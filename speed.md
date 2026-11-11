# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b57b83c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,406 | 15,214 | 0.9469× | 0.94× | ✅ |
| echo-1k | 17,534 | 18,169 | 0.9651× | 0.96× | ✅ |
| json | 22,489 | 23,170 | 0.9706× | 0.97× | ✅ |
| service-health | 16,991 | 17,786 | 0.9553× | 0.95× | ✅ |
| websocket | 6,855 | 8,188 | 0.8373× | 0.84× | ✅ |
| timer | 202,811 | 313,522 | 0.6469× | 0.65× | ✅ |
| app | 126,703 | 135,907 | 0.9323× | 0.94× | ✅ |
| http2 | 20,311 | 20,467 | 0.9924× | 0.99× | ✅ |
| table | 196,338 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,698 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,369 | 33,274 | 0.9728× | 0.97× | ✅ |
| mysql | 50,016 | 51,064 | 0.9795× | 0.98× | ✅ |
| http-client | 34,215 | 35,825 | 0.9550× | 0.96× | ✅ |
| client | 18,909 | 19,765 | 0.9567× | 0.95× | ✅ |
| socket | 20,372 | 21,071 | 0.9668× | 0.97× | ✅ |
| gzip | 196,402 | 197,086 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,765 | 1.1261× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,986 | 0.9995× | 1.0× | ✅ |
| connect | 19,241 | 18,707 | 1.0286× | 1.03× | ➖ |
| https | 109,615 | 108,806 | 1.0074× | 1.01× | ➖ |
| wss | 105,064 | 104,805 | 1.0025× | 1.0× | ❌ |
| atomic | 50,214 | 51,325 | 0.9784× | 0.98× | ✅ |
| process-mode | 16,667 | 17,028 | 0.9788× | 0.98× | ✅ |
| http2-concurrent | 377,657 | 409,073 | 0.9232× | 0.92× | ✅ |
| websocket-64k | 156,305 | 159,699 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,009 | 16,358 | 0.8564× | 0.86× | ✅ |
| upload | 674,101 | 1,322,449 | 0.5097× | 0.51× | ✅ |
| static | 35,215 | 41,103 | 0.8567× | 0.86× | ✅ |
| pgsql | 23,695 | 24,594 | 0.9634× | 0.97× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 0.96× | ✅ |
| curl | 86,359 | 90,518 | 0.9541× | 0.96× | ✅ |
| dns | 231,597 | 233,170 | 0.9933× | 1.0× | ✅ |

Geometric mean 0.9201×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
