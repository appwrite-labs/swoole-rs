# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `080fa84` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,477 | 15,210 | 0.9518× | 0.94× | ✅ |
| echo-1k | 17,603 | 18,162 | 0.9692× | 0.97× | ✅ |
| json | 22,563 | 23,167 | 0.9739× | 0.97× | ✅ |
| service-health | 17,056 | 17,786 | 0.9590× | 0.95× | ✅ |
| websocket | 6,913 | 8,188 | 0.8443× | 0.84× | ✅ |
| timer | 202,653 | 313,601 | 0.6462× | 0.65× | ✅ |
| app | 126,736 | 135,947 | 0.9322× | 0.94× | ✅ |
| http2 | 20,408 | 20,468 | 0.9971× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,756 | 154,667 | 0.9036× | 0.91× | ✅ |
| task | 32,441 | 33,265 | 0.9752× | 0.97× | ✅ |
| mysql | 50,056 | 51,064 | 0.9802× | 0.98× | ✅ |
| http-client | 34,309 | 35,827 | 0.9576× | 0.96× | ✅ |
| client | 18,971 | 19,766 | 0.9597× | 0.95× | ✅ |
| socket | 20,434 | 21,071 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,986 | 1.0039× | 1.0× | ❌ |
| connect | 18,908 | 18,707 | 1.0107× | 1.02× | ➖ |
| https | 109,665 | 108,803 | 1.0079× | 1.01× | ➖ |
| wss | 105,073 | 104,805 | 1.0026× | 1.0× | ❌ |
| atomic | 50,276 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,741 | 16,959 | 0.9871× | 0.98× | ✅ |
| http2-concurrent | 379,026 | 409,538 | 0.9255× | 0.92× | ✅ |
| websocket-64k | 156,358 | 159,709 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,885 | 16,335 | 0.7888× | 0.79× | ✅ |
| upload | 675,601 | 1,322,382 | 0.5109× | 0.51× | ✅ |
| static | 34,729 | 40,987 | 0.8473× | 0.85× | ✅ |
| pgsql | 23,771 | 24,595 | 0.9665× | 0.97× | ✅ |
| sqlite | 24,076 | 25,232 | 0.9542× | 0.96× | ✅ |
| curl | 86,105 | 90,514 | 0.9513× | 0.96× | ✅ |
| dns | 231,785 | 232,916 | 0.9951× | 1.0× | ✅ |

Geometric mean 0.9196×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
