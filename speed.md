# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `58ab4b6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,457 | 15,210 | 0.9505× | 0.94× | ✅ |
| echo-1k | 17,616 | 18,165 | 0.9698× | 0.97× | ✅ |
| json | 22,562 | 23,170 | 0.9737× | 0.97× | ✅ |
| service-health | 17,056 | 17,782 | 0.9592× | 0.95× | ✅ |
| websocket | 6,913 | 8,188 | 0.8442× | 0.84× | ✅ |
| timer | 202,553 | 313,561 | 0.6460× | 0.65× | ✅ |
| app | 126,720 | 135,957 | 0.9321× | 0.94× | ✅ |
| http2 | 20,404 | 20,465 | 0.9971× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,756 | 154,667 | 0.9036× | 0.91× | ✅ |
| task | 32,419 | 33,261 | 0.9747× | 0.97× | ✅ |
| mysql | 50,114 | 51,061 | 0.9815× | 0.98× | ✅ |
| http-client | 34,309 | 35,830 | 0.9576× | 0.96× | ✅ |
| client | 18,970 | 19,765 | 0.9598× | 0.95× | ✅ |
| socket | 20,434 | 21,071 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,986 | 1.0039× | 1.0× | ❌ |
| connect | 18,908 | 18,707 | 1.0107× | 1.03× | ➖ |
| https | 109,671 | 108,810 | 1.0079× | 1.01× | ➖ |
| wss | 105,091 | 104,802 | 1.0028× | 1.0× | ❌ |
| atomic | 50,276 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,779 | 16,957 | 0.9895× | 0.98× | ✅ |
| http2-concurrent | 379,721 | 409,072 | 0.9282× | 0.92× | ✅ |
| websocket-64k | 156,357 | 159,705 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,335 | 0.7885× | 0.79× | ✅ |
| upload | 675,608 | 1,322,382 | 0.5109× | 0.51× | ✅ |
| static | 34,788 | 41,074 | 0.8470× | 0.86× | ✅ |
| pgsql | 23,769 | 24,595 | 0.9664× | 0.97× | ✅ |
| sqlite | 24,070 | 25,232 | 0.9540× | 0.96× | ✅ |
| curl | 86,431 | 90,381 | 0.9563× | 0.96× | ✅ |
| dns | 231,398 | 233,111 | 0.9927× | 1.0× | ✅ |

Geometric mean 0.9198×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
