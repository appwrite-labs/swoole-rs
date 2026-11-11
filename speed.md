# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `954e71e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,406 | 15,214 | 0.9469× | 0.94× | ✅ |
| echo-1k | 17,528 | 18,172 | 0.9646× | 0.96× | ✅ |
| json | 22,489 | 23,170 | 0.9706× | 0.97× | ✅ |
| service-health | 16,991 | 17,786 | 0.9553× | 0.95× | ✅ |
| websocket | 6,855 | 8,188 | 0.8373× | 0.84× | ✅ |
| timer | 202,798 | 313,575 | 0.6467× | 0.65× | ✅ |
| app | 126,697 | 135,938 | 0.9320× | 0.94× | ✅ |
| http2 | 20,303 | 20,467 | 0.9920× | 0.99× | ✅ |
| table | 196,338 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,694 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,368 | 33,265 | 0.9730× | 0.97× | ✅ |
| mysql | 50,039 | 51,064 | 0.9799× | 0.98× | ✅ |
| http-client | 34,207 | 35,830 | 0.9547× | 0.96× | ✅ |
| client | 18,909 | 19,766 | 0.9567× | 0.95× | ✅ |
| socket | 20,372 | 21,071 | 0.9668× | 0.97× | ✅ |
| gzip | 196,408 | 197,088 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,636 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,241 | 18,707 | 1.0286× | 1.03× | ➖ |
| https | 109,615 | 108,803 | 1.0075× | 1.01× | ➖ |
| wss | 105,050 | 104,809 | 1.0023× | 1.0× | ❌ |
| atomic | 50,214 | 51,325 | 0.9784× | 0.98× | ✅ |
| process-mode | 16,671 | 16,963 | 0.9828× | 0.98× | ✅ |
| http2-concurrent | 377,268 | 409,072 | 0.9223× | 0.92× | ✅ |
| websocket-64k | 156,303 | 159,702 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,009 | 16,359 | 0.8564× | 0.86× | ✅ |
| upload | 674,125 | 1,322,421 | 0.5098× | 0.51× | ✅ |
| static | 35,145 | 41,045 | 0.8562× | 0.86× | ✅ |
| pgsql | 23,684 | 24,595 | 0.9630× | 0.97× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 0.96× | ✅ |
| curl | 86,394 | 90,411 | 0.9556× | 0.96× | ✅ |
| dns | 231,692 | 233,018 | 0.9943× | 1.0× | ✅ |

Geometric mean 0.9202×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
