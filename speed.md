# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9d925f8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,405 | 15,213 | 0.9469× | 0.94× | ✅ |
| echo-1k | 17,524 | 18,186 | 0.9636× | 0.96× | ✅ |
| json | 22,485 | 23,170 | 0.9704× | 0.97× | ✅ |
| service-health | 16,991 | 17,786 | 0.9553× | 0.95× | ✅ |
| websocket | 6,859 | 8,188 | 0.8377× | 0.84× | ✅ |
| timer | 202,788 | 313,622 | 0.6466× | 0.65× | ✅ |
| app | 126,669 | 135,929 | 0.9319× | 0.94× | ✅ |
| http2 | 20,324 | 20,468 | 0.9930× | 0.99× | ✅ |
| table | 196,342 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,694 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,362 | 33,264 | 0.9729× | 0.97× | ✅ |
| mysql | 50,033 | 51,024 | 0.9806× | 0.98× | ✅ |
| http-client | 34,217 | 35,833 | 0.9549× | 0.96× | ✅ |
| client | 18,909 | 19,763 | 0.9568× | 0.95× | ✅ |
| socket | 20,368 | 21,070 | 0.9667× | 0.97× | ✅ |
| gzip | 196,409 | 197,095 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,228 | 0.5730× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,238 | 18,707 | 1.0284× | 1.03× | ➖ |
| https | 109,599 | 108,803 | 1.0073× | 1.01× | ➖ |
| wss | 105,064 | 104,802 | 1.0025× | 1.0× | ❌ |
| atomic | 50,214 | 51,325 | 0.9784× | 0.98× | ✅ |
| process-mode | 16,670 | 16,957 | 0.9831× | 0.98× | ✅ |
| http2-concurrent | 377,351 | 409,073 | 0.9225× | 0.92× | ✅ |
| websocket-64k | 156,303 | 159,702 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,009 | 16,359 | 0.8564× | 0.86× | ✅ |
| upload | 674,110 | 1,322,453 | 0.5097× | 0.51× | ✅ |
| static | 35,157 | 41,074 | 0.8559× | 0.86× | ✅ |
| pgsql | 23,695 | 24,594 | 0.9635× | 0.97× | ✅ |
| sqlite | 23,998 | 25,217 | 0.9517× | 0.96× | ✅ |
| curl | 86,360 | 90,364 | 0.9557× | 0.96× | ✅ |
| dns | 231,397 | 232,969 | 0.9933× | 1.0× | ✅ |

Geometric mean 0.9202×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
