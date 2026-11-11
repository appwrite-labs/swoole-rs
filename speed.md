# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f367f94` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,475 | 15,213 | 0.9514× | 0.94× | ✅ |
| echo-1k | 17,619 | 18,165 | 0.9699× | 0.97× | ✅ |
| json | 22,582 | 23,167 | 0.9747× | 0.97× | ✅ |
| service-health | 17,076 | 17,786 | 0.9601× | 0.95× | ✅ |
| websocket | 6,912 | 8,188 | 0.8441× | 0.84× | ✅ |
| timer | 202,259 | 313,683 | 0.6448× | 0.65× | ✅ |
| app | 126,717 | 135,920 | 0.9323× | 0.94× | ✅ |
| http2 | 20,416 | 20,464 | 0.9977× | 0.99× | ✅ |
| table | 196,359 | 201,874 | 0.9727× | 0.98× | ✅ |
| channel | 139,780 | 154,664 | 0.9038× | 0.91× | ✅ |
| task | 32,460 | 33,268 | 0.9757× | 0.97× | ✅ |
| mysql | 50,115 | 51,061 | 0.9815× | 0.98× | ✅ |
| http-client | 34,323 | 35,829 | 0.9580× | 0.96× | ✅ |
| client | 18,988 | 19,765 | 0.9607× | 0.95× | ✅ |
| socket | 20,454 | 21,071 | 0.9707× | 0.97× | ✅ |
| gzip | 196,476 | 197,079 | 0.9969× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,765 | 1.1418× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,986 | 1.0040× | 1.0× | ❌ |
| connect | 18,927 | 18,707 | 1.0117× | 1.02× | ➖ |
| https | 109,699 | 108,810 | 1.0082× | 1.01× | ➖ |
| wss | 105,077 | 104,808 | 1.0026× | 1.0× | ❌ |
| atomic | 50,296 | 51,325 | 0.9800× | 0.98× | ✅ |
| process-mode | 16,780 | 16,959 | 0.9894× | 0.98× | ✅ |
| http2-concurrent | 379,089 | 409,069 | 0.9267× | 0.92× | ✅ |
| websocket-64k | 156,358 | 159,703 | 0.9791× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,339 | 0.7884× | 0.79× | ✅ |
| upload | 675,625 | 1,322,380 | 0.5109× | 0.51× | ✅ |
| static | 34,714 | 40,987 | 0.8469× | 0.85× | ✅ |
| pgsql | 23,760 | 24,594 | 0.9661× | 0.97× | ✅ |
| sqlite | 24,064 | 25,232 | 0.9537× | 0.96× | ✅ |
| curl | 86,423 | 90,432 | 0.9557× | 0.96× | ✅ |
| dns | 231,701 | 233,081 | 0.9941× | 1.0× | ✅ |

Geometric mean 0.9199×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
