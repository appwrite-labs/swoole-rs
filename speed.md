# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d5124ad` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,172 | 15,210 | 0.9318× | 0.94× | ✅ |
| echo-1k | 17,271 | 18,162 | 0.9510× | 0.97× | ✅ |
| json | 22,249 | 23,167 | 0.9604× | 0.97× | ✅ |
| service-health | 16,743 | 17,782 | 0.9416× | 0.95× | ✅ |
| websocket | 6,826 | 8,188 | 0.8337× | 0.84× | ✅ |
| timer | 201,997 | 313,516 | 0.6443× | 0.65× | ✅ |
| app | 126,333 | 135,931 | 0.9294× | 0.93× | ✅ |
| http2 | 20,129 | 20,464 | 0.9836× | 0.99× | ✅ |
| table | 195,919 | 201,874 | 0.9705× | 0.98× | ✅ |
| channel | 139,435 | 154,667 | 0.9015× | 0.91× | ✅ |
| task | 32,070 | 33,264 | 0.9641× | 0.97× | ✅ |
| mysql | 49,683 | 51,044 | 0.9733× | 0.98× | ✅ |
| http-client | 33,820 | 35,830 | 0.9439× | 0.96× | ✅ |
| client | 18,557 | 19,763 | 0.9390× | 0.95× | ✅ |
| socket | 19,598 | 21,071 | 0.9301× | 0.97× | ✅ |
| gzip | 196,119 | 197,083 | 0.9951× | 1.0× | ✅ |
| websocket-deflate | 181,596 | 181,572 | 1.0001× | 1.0× | ❌ |
| tcp-length | 5,372 | 4,762 | 1.1281× | 1.13× | ➖ |
| tcp-eof | 5,881 | 10,231 | 0.5748× | 0.58× | ✅ |
| udp | 8,025 | 7,987 | 1.0048× | 1.0× | ❌ |
| connect | 18,582 | 18,703 | 0.9935× | 0.99× | ✅ |
| https | 109,245 | 108,813 | 1.0040× | 1.01× | ➖ |
| wss | 104,897 | 104,805 | 1.0009× | 1.0× | ❌ |
| atomic | 49,954 | 51,325 | 0.9733× | 0.98× | ✅ |
| process-mode | 16,471 | 16,962 | 0.9710× | 0.98× | ✅ |
| http2-concurrent | 377,246 | 409,073 | 0.9222× | 0.92× | ✅ |
| websocket-64k | 156,277 | 159,699 | 0.9786× | 0.98× | ✅ |
| websocket-fragmented | 13,040 | 16,335 | 0.7983× | 0.79× | ✅ |
| upload | 544,331 | 1,322,380 | 0.4116× | 0.51× | ✅ |
| static | 34,668 | 41,010 | 0.8453× | 0.85× | ✅ |
| pgsql | 23,459 | 24,598 | 0.9537× | 0.97× | ✅ |
| sqlite | 23,795 | 25,232 | 0.9431× | 0.96× | ✅ |
| curl | 85,765 | 90,509 | 0.9476× | 0.96× | ✅ |
| dns | 231,719 | 232,753 | 0.9956× | 1.0× | ✅ |

Geometric mean 0.9058×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
