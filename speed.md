# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b34604c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,456 | 15,213 | 0.9502× | 0.94× | ✅ |
| echo-1k | 17,616 | 18,165 | 0.9698× | 0.97× | ✅ |
| json | 22,562 | 23,170 | 0.9737× | 0.97× | ✅ |
| service-health | 17,056 | 17,786 | 0.9590× | 0.95× | ✅ |
| websocket | 6,913 | 8,188 | 0.8443× | 0.84× | ✅ |
| timer | 202,600 | 313,620 | 0.6460× | 0.65× | ✅ |
| app | 126,725 | 135,879 | 0.9326× | 0.94× | ✅ |
| http2 | 20,409 | 20,464 | 0.9973× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,756 | 154,664 | 0.9036× | 0.91× | ✅ |
| task | 32,419 | 33,261 | 0.9747× | 0.97× | ✅ |
| mysql | 50,113 | 51,058 | 0.9815× | 0.98× | ✅ |
| http-client | 34,305 | 35,827 | 0.9575× | 0.96× | ✅ |
| client | 18,971 | 19,766 | 0.9598× | 0.95× | ✅ |
| socket | 20,433 | 21,070 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,690 | 181,572 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,986 | 1.0039× | 1.0× | ❌ |
| connect | 18,904 | 18,707 | 1.0105× | 1.02× | ➖ |
| https | 109,656 | 108,803 | 1.0078× | 1.01× | ➖ |
| wss | 105,077 | 104,802 | 1.0026× | 1.0× | ❌ |
| atomic | 50,273 | 51,325 | 0.9795× | 0.98× | ✅ |
| process-mode | 16,778 | 16,962 | 0.9891× | 0.98× | ✅ |
| http2-concurrent | 379,565 | 409,073 | 0.9279× | 0.92× | ✅ |
| websocket-64k | 156,354 | 159,705 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,885 | 16,335 | 0.7888× | 0.79× | ✅ |
| upload | 675,608 | 1,322,382 | 0.5109× | 0.51× | ✅ |
| static | 34,784 | 41,045 | 0.8475× | 0.85× | ✅ |
| pgsql | 23,764 | 24,598 | 0.9661× | 0.97× | ✅ |
| sqlite | 24,074 | 25,232 | 0.9541× | 0.96× | ✅ |
| curl | 86,432 | 90,383 | 0.9563× | 0.96× | ✅ |
| dns | 231,377 | 232,822 | 0.9938× | 1.0× | ✅ |

Geometric mean 0.9198×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
