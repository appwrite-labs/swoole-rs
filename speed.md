# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e6e6d4c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,065 | 15,214 | 0.9245× | 0.94× | ✅ |
| echo-1k | 17,185 | 18,165 | 0.9460× | 0.97× | ✅ |
| json | 22,143 | 23,170 | 0.9557× | 0.97× | ✅ |
| service-health | 16,636 | 17,786 | 0.9354× | 0.95× | ✅ |
| websocket | 6,716 | 8,188 | 0.8202× | 0.84× | ✅ |
| timer | 202,079 | 313,630 | 0.6443× | 0.65× | ✅ |
| app | 126,320 | 135,830 | 0.9300× | 0.93× | ✅ |
| http2 | 19,923 | 20,468 | 0.9734× | 0.99× | ✅ |
| table | 195,306 | 201,874 | 0.9675× | 0.98× | ✅ |
| channel | 139,517 | 154,667 | 0.9020× | 0.91× | ✅ |
| task | 31,963 | 33,268 | 0.9608× | 0.97× | ✅ |
| mysql | 49,507 | 51,064 | 0.9695× | 0.98× | ✅ |
| http-client | 33,777 | 35,829 | 0.9427× | 0.96× | ✅ |
| client | 18,470 | 19,766 | 0.9344× | 0.95× | ✅ |
| socket | 19,502 | 21,071 | 0.9256× | 0.97× | ✅ |
| gzip | 196,022 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,485 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,231 | 4,762 | 1.0984× | 1.13× | ➖ |
| tcp-eof | 5,801 | 10,231 | 0.5670× | 0.58× | ✅ |
| udp | 7,986 | 7,987 | 1.0000× | 1.0× | ✅ |
| connect | 18,335 | 18,707 | 0.9801× | 0.99× | ✅ |
| https | 109,091 | 108,810 | 1.0026× | 1.01× | ➖ |
| wss | 104,738 | 104,802 | 0.9994× | 1.0× | ✅ |
| atomic | 49,857 | 51,325 | 0.9714× | 0.98× | ✅ |
| process-mode | 16,389 | 17,024 | 0.9627× | 0.98× | ✅ |
| http2-concurrent | 373,596 | 409,074 | 0.9133× | 0.92× | ✅ |
| websocket-64k | 156,167 | 159,702 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 12,965 | 16,339 | 0.7935× | 0.79× | ✅ |
| upload | 544,227 | 1,322,381 | 0.4116× | 0.51× | ✅ |
| static | 34,551 | 40,981 | 0.8431× | 0.85× | ✅ |
| pgsql | 23,360 | 24,595 | 0.9498× | 0.97× | ✅ |
| sqlite | 23,699 | 25,232 | 0.9393× | 0.96× | ✅ |
| curl | 85,688 | 90,299 | 0.9489× | 0.96× | ✅ |
| dns | 231,582 | 233,164 | 0.9932× | 1.0× | ✅ |

Geometric mean 0.9012×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
