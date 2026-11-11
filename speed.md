# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3da360e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,453 | 15,210 | 0.9502× | 0.94× | ✅ |
| echo-1k | 17,585 | 18,166 | 0.9680× | 0.97× | ✅ |
| json | 22,567 | 23,170 | 0.9740× | 0.97× | ✅ |
| service-health | 17,057 | 17,782 | 0.9592× | 0.95× | ✅ |
| websocket | 6,913 | 8,191 | 0.8439× | 0.84× | ✅ |
| timer | 202,589 | 313,593 | 0.6460× | 0.65× | ✅ |
| app | 126,720 | 135,910 | 0.9324× | 0.94× | ✅ |
| http2 | 20,409 | 20,465 | 0.9973× | 0.99× | ✅ |
| table | 196,212 | 201,878 | 0.9719× | 0.98× | ✅ |
| channel | 139,761 | 154,664 | 0.9036× | 0.91× | ✅ |
| task | 32,421 | 33,268 | 0.9745× | 0.97× | ✅ |
| mysql | 49,958 | 51,061 | 0.9784× | 0.98× | ✅ |
| http-client | 34,305 | 35,829 | 0.9575× | 0.96× | ✅ |
| client | 18,972 | 19,767 | 0.9598× | 0.95× | ✅ |
| socket | 20,435 | 21,071 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,765 | 1.1418× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0039× | 1.0× | ❌ |
| connect | 18,904 | 18,707 | 1.0105× | 1.02× | ➖ |
| https | 109,639 | 108,810 | 1.0076× | 1.01× | ➖ |
| wss | 105,077 | 104,802 | 1.0026× | 1.0× | ❌ |
| atomic | 50,277 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,763 | 17,021 | 0.9849× | 0.98× | ✅ |
| http2-concurrent | 378,958 | 409,445 | 0.9255× | 0.92× | ✅ |
| websocket-64k | 156,365 | 159,705 | 0.9791× | 0.98× | ✅ |
| websocket-fragmented | 12,880 | 16,335 | 0.7885× | 0.79× | ✅ |
| upload | 675,606 | 1,322,381 | 0.5109× | 0.51× | ✅ |
| static | 34,747 | 40,987 | 0.8478× | 0.85× | ✅ |
| pgsql | 23,751 | 24,595 | 0.9657× | 0.97× | ✅ |
| sqlite | 24,058 | 25,232 | 0.9535× | 0.96× | ✅ |
| curl | 86,091 | 90,498 | 0.9513× | 0.96× | ✅ |
| dns | 231,559 | 232,647 | 0.9953× | 1.0× | ✅ |

Geometric mean 0.9193×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
