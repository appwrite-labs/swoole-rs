# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4e7d932` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,409 | 15,213 | 0.9472× | 0.94× | ✅ |
| echo-1k | 17,526 | 18,168 | 0.9647× | 0.96× | ✅ |
| json | 22,485 | 23,170 | 0.9704× | 0.97× | ✅ |
| service-health | 16,991 | 17,786 | 0.9553× | 0.95× | ✅ |
| websocket | 6,855 | 8,184 | 0.8375× | 0.84× | ✅ |
| timer | 202,892 | 313,571 | 0.6470× | 0.65× | ✅ |
| app | 126,687 | 135,854 | 0.9325× | 0.94× | ✅ |
| http2 | 20,326 | 20,468 | 0.9931× | 0.99× | ✅ |
| table | 196,342 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,694 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,368 | 33,272 | 0.9729× | 0.97× | ✅ |
| mysql | 50,030 | 51,062 | 0.9798× | 0.98× | ✅ |
| http-client | 34,210 | 35,827 | 0.9549× | 0.96× | ✅ |
| client | 18,909 | 19,767 | 0.9566× | 0.95× | ✅ |
| socket | 20,372 | 21,071 | 0.9668× | 0.97× | ✅ |
| gzip | 196,411 | 197,090 | 0.9966× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,228 | 0.5730× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9995× | 1.0× | ✅ |
| connect | 19,241 | 18,707 | 1.0286× | 1.03× | ➖ |
| https | 109,619 | 108,814 | 1.0074× | 1.01× | ➖ |
| wss | 105,065 | 104,808 | 1.0025× | 1.0× | ❌ |
| atomic | 50,214 | 51,325 | 0.9784× | 0.98× | ✅ |
| process-mode | 16,671 | 16,956 | 0.9832× | 0.98× | ✅ |
| http2-concurrent | 377,577 | 409,078 | 0.9230× | 0.92× | ✅ |
| websocket-64k | 156,304 | 159,708 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,008 | 16,358 | 0.8564× | 0.86× | ✅ |
| upload | 674,130 | 1,322,445 | 0.5098× | 0.51× | ✅ |
| static | 35,174 | 41,074 | 0.8563× | 0.86× | ✅ |
| pgsql | 23,694 | 24,595 | 0.9633× | 0.97× | ✅ |
| sqlite | 24,003 | 25,217 | 0.9518× | 0.96× | ✅ |
| curl | 86,354 | 90,381 | 0.9554× | 0.96× | ✅ |
| dns | 231,431 | 232,728 | 0.9944× | 1.0× | ✅ |

Geometric mean 0.9203×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
