# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e5825e7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,156 | 15,214 | 0.9305× | 0.94× | ✅ |
| echo-1k | 17,255 | 18,165 | 0.9499× | 0.97× | ✅ |
| json | 22,239 | 23,167 | 0.9600× | 0.97× | ✅ |
| service-health | 16,731 | 17,786 | 0.9407× | 0.95× | ✅ |
| websocket | 6,825 | 8,188 | 0.8336× | 0.84× | ✅ |
| timer | 202,447 | 314,479 | 0.6438× | 0.65× | ✅ |
| app | 126,323 | 135,954 | 0.9292× | 0.93× | ✅ |
| http2 | 20,209 | 20,464 | 0.9875× | 0.99× | ✅ |
| table | 195,910 | 201,878 | 0.9704× | 0.98× | ✅ |
| channel | 139,431 | 154,667 | 0.9015× | 0.91× | ✅ |
| task | 32,002 | 33,265 | 0.9620× | 0.97× | ✅ |
| mysql | 49,672 | 51,029 | 0.9734× | 0.98× | ✅ |
| http-client | 33,782 | 35,829 | 0.9429× | 0.96× | ✅ |
| client | 18,552 | 19,765 | 0.9386× | 0.95× | ✅ |
| socket | 19,590 | 21,070 | 0.9298× | 0.97× | ✅ |
| gzip | 196,110 | 197,083 | 0.9951× | 1.0× | ✅ |
| websocket-deflate | 181,596 | 181,572 | 1.0001× | 1.0× | ❌ |
| tcp-length | 5,372 | 4,762 | 1.1281× | 1.13× | ➖ |
| tcp-eof | 5,881 | 10,231 | 0.5748× | 0.58× | ✅ |
| udp | 8,025 | 7,987 | 1.0048× | 1.0× | ❌ |
| connect | 18,570 | 18,703 | 0.9929× | 0.99× | ✅ |
| https | 109,225 | 108,813 | 1.0038× | 1.01× | ➖ |
| wss | 104,897 | 104,805 | 1.0009× | 1.0× | ❌ |
| atomic | 49,942 | 51,325 | 0.9731× | 0.98× | ✅ |
| process-mode | 16,457 | 17,021 | 0.9669× | 0.98× | ✅ |
| http2-concurrent | 379,718 | 409,072 | 0.9282× | 0.92× | ✅ |
| websocket-64k | 156,273 | 159,701 | 0.9785× | 0.98× | ✅ |
| websocket-fragmented | 13,036 | 16,335 | 0.7980× | 0.79× | ✅ |
| upload | 544,344 | 1,322,383 | 0.4116× | 0.51× | ✅ |
| static | 34,639 | 40,989 | 0.8451× | 0.85× | ✅ |
| pgsql | 23,441 | 24,594 | 0.9531× | 0.97× | ✅ |
| sqlite | 23,778 | 25,232 | 0.9424× | 0.96× | ✅ |
| curl | 85,744 | 90,510 | 0.9474× | 0.96× | ✅ |
| dns | 231,890 | 232,845 | 0.9959× | 1.0× | ✅ |

Geometric mean 0.9057×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
