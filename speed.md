# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a96c828` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,456 | 15,214 | 0.9502× | 0.94× | ✅ |
| echo-1k | 17,617 | 18,165 | 0.9698× | 0.97× | ✅ |
| json | 22,565 | 23,170 | 0.9739× | 0.97× | ✅ |
| service-health | 17,059 | 17,786 | 0.9591× | 0.95× | ✅ |
| websocket | 6,908 | 8,188 | 0.8437× | 0.84× | ✅ |
| timer | 202,946 | 314,510 | 0.6453× | 0.65× | ✅ |
| app | 126,724 | 135,973 | 0.9320× | 0.94× | ✅ |
| http2 | 20,406 | 20,464 | 0.9972× | 0.99× | ✅ |
| table | 196,345 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,767 | 154,667 | 0.9037× | 0.91× | ✅ |
| task | 32,419 | 33,268 | 0.9745× | 0.97× | ✅ |
| mysql | 50,105 | 51,064 | 0.9812× | 0.98× | ✅ |
| http-client | 34,315 | 35,830 | 0.9577× | 0.96× | ✅ |
| client | 18,974 | 19,765 | 0.9599× | 0.95× | ✅ |
| socket | 20,440 | 21,071 | 0.9701× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,572 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,765 | 1.1418× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,017 | 7,987 | 1.0038× | 1.0× | ❌ |
| connect | 18,908 | 18,703 | 1.0109× | 1.02× | ➖ |
| https | 109,667 | 108,813 | 1.0078× | 1.01× | ➖ |
| wss | 105,087 | 104,802 | 1.0027× | 1.0× | ❌ |
| atomic | 50,279 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,763 | 16,960 | 0.9884× | 0.98× | ✅ |
| http2-concurrent | 378,960 | 409,376 | 0.9257× | 0.92× | ✅ |
| websocket-64k | 156,354 | 159,701 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,335 | 0.7885× | 0.79× | ✅ |
| upload | 675,599 | 1,322,382 | 0.5109× | 0.51× | ✅ |
| static | 34,784 | 41,045 | 0.8475× | 0.85× | ✅ |
| pgsql | 23,768 | 24,594 | 0.9664× | 0.97× | ✅ |
| sqlite | 24,074 | 25,232 | 0.9541× | 0.96× | ✅ |
| curl | 86,433 | 90,320 | 0.9570× | 0.96× | ✅ |
| dns | 231,407 | 233,153 | 0.9925× | 1.0× | ✅ |

Geometric mean 0.9197×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
