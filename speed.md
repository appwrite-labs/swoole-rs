# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4fd26a1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,457 | 15,213 | 0.9503× | 0.94× | ✅ |
| echo-1k | 17,613 | 18,166 | 0.9696× | 0.97× | ✅ |
| json | 22,558 | 23,170 | 0.9736× | 0.97× | ✅ |
| service-health | 17,056 | 17,782 | 0.9592× | 0.95× | ✅ |
| websocket | 6,908 | 8,191 | 0.8433× | 0.84× | ✅ |
| timer | 202,501 | 313,529 | 0.6459× | 0.65× | ✅ |
| app | 126,702 | 135,908 | 0.9323× | 0.94× | ✅ |
| http2 | 20,412 | 20,465 | 0.9974× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,756 | 154,667 | 0.9036× | 0.91× | ✅ |
| task | 32,423 | 33,261 | 0.9748× | 0.97× | ✅ |
| mysql | 50,110 | 51,049 | 0.9816× | 0.98× | ✅ |
| http-client | 34,305 | 35,830 | 0.9574× | 0.96× | ✅ |
| client | 18,971 | 19,762 | 0.9599× | 0.95× | ✅ |
| socket | 20,433 | 21,071 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,017 | 7,986 | 1.0039× | 1.0× | ❌ |
| connect | 18,904 | 18,707 | 1.0106× | 1.02× | ➖ |
| https | 109,676 | 108,810 | 1.0080× | 1.01× | ➖ |
| wss | 105,092 | 104,808 | 1.0027× | 1.0× | ❌ |
| atomic | 50,276 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,779 | 17,025 | 0.9855× | 0.98× | ✅ |
| http2-concurrent | 379,539 | 409,078 | 0.9278× | 0.92× | ✅ |
| websocket-64k | 156,362 | 159,707 | 0.9791× | 0.98× | ✅ |
| websocket-fragmented | 12,885 | 16,335 | 0.7888× | 0.79× | ✅ |
| upload | 675,613 | 1,322,384 | 0.5109× | 0.51× | ✅ |
| static | 34,755 | 41,016 | 0.8473× | 0.85× | ✅ |
| pgsql | 23,767 | 24,594 | 0.9664× | 0.97× | ✅ |
| sqlite | 24,074 | 25,232 | 0.9541× | 0.96× | ✅ |
| curl | 86,479 | 90,385 | 0.9568× | 0.96× | ✅ |
| dns | 231,782 | 232,940 | 0.9950× | 1.0× | ✅ |

Geometric mean 0.9198×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
