# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e253ef7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,459 | 15,210 | 0.9506× | 0.94× | ✅ |
| echo-1k | 17,616 | 18,162 | 0.9699× | 0.97× | ✅ |
| json | 22,558 | 23,170 | 0.9736× | 0.97× | ✅ |
| service-health | 17,053 | 17,786 | 0.9588× | 0.95× | ✅ |
| websocket | 6,908 | 8,188 | 0.8437× | 0.84× | ✅ |
| timer | 202,807 | 313,617 | 0.6467× | 0.65× | ✅ |
| app | 126,730 | 135,965 | 0.9321× | 0.94× | ✅ |
| http2 | 20,408 | 20,464 | 0.9972× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,760 | 154,664 | 0.9036× | 0.91× | ✅ |
| task | 32,418 | 33,261 | 0.9747× | 0.97× | ✅ |
| mysql | 50,110 | 51,058 | 0.9814× | 0.98× | ✅ |
| http-client | 34,306 | 35,830 | 0.9575× | 0.96× | ✅ |
| client | 18,971 | 19,766 | 0.9598× | 0.95× | ✅ |
| socket | 20,434 | 21,071 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,765 | 1.1418× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0039× | 1.0× | ❌ |
| connect | 18,908 | 18,707 | 1.0107× | 1.02× | ➖ |
| https | 109,666 | 108,803 | 1.0079× | 1.01× | ➖ |
| wss | 105,077 | 104,802 | 1.0026× | 1.0× | ❌ |
| atomic | 50,276 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,781 | 16,959 | 0.9895× | 0.98× | ✅ |
| http2-concurrent | 379,433 | 409,074 | 0.9275× | 0.92× | ✅ |
| websocket-64k | 156,364 | 159,704 | 0.9791× | 0.98× | ✅ |
| websocket-fragmented | 12,885 | 16,339 | 0.7886× | 0.79× | ✅ |
| upload | 675,608 | 1,322,380 | 0.5109× | 0.51× | ✅ |
| static | 34,813 | 41,074 | 0.8476× | 0.85× | ✅ |
| pgsql | 23,767 | 24,598 | 0.9662× | 0.97× | ✅ |
| sqlite | 24,074 | 25,232 | 0.9541× | 0.96× | ✅ |
| curl | 86,435 | 90,595 | 0.9541× | 0.96× | ✅ |
| dns | 231,750 | 233,024 | 0.9945× | 1.0× | ✅ |

Geometric mean 0.9198×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
