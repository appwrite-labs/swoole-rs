# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2b430a7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,455 | 15,213 | 0.9501× | 0.94× | ✅ |
| echo-1k | 17,589 | 18,162 | 0.9685× | 0.97× | ✅ |
| json | 22,561 | 23,167 | 0.9739× | 0.97× | ✅ |
| service-health | 17,056 | 17,786 | 0.9590× | 0.95× | ✅ |
| websocket | 6,913 | 8,188 | 0.8443× | 0.84× | ✅ |
| timer | 202,578 | 313,581 | 0.6460× | 0.65× | ✅ |
| app | 126,724 | 135,962 | 0.9321× | 0.94× | ✅ |
| http2 | 20,444 | 20,464 | 0.9990× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,756 | 154,667 | 0.9036× | 0.91× | ✅ |
| task | 32,419 | 33,265 | 0.9746× | 0.97× | ✅ |
| mysql | 50,073 | 51,064 | 0.9806× | 0.98× | ✅ |
| http-client | 34,303 | 35,829 | 0.9574× | 0.96× | ✅ |
| client | 18,972 | 19,766 | 0.9598× | 0.95× | ✅ |
| socket | 20,434 | 21,071 | 0.9698× | 0.97× | ✅ |
| gzip | 196,461 | 197,083 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0039× | 1.0× | ❌ |
| connect | 18,908 | 18,707 | 1.0107× | 1.03× | ➖ |
| https | 109,680 | 108,813 | 1.0080× | 1.01× | ➖ |
| wss | 105,087 | 104,805 | 1.0027× | 1.0× | ❌ |
| atomic | 50,276 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,757 | 17,027 | 0.9841× | 0.98× | ✅ |
| http2-concurrent | 379,331 | 409,072 | 0.9273× | 0.92× | ✅ |
| websocket-64k | 156,355 | 159,703 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,880 | 16,335 | 0.7885× | 0.79× | ✅ |
| upload | 675,610 | 1,322,380 | 0.5109× | 0.51× | ✅ |
| static | 34,851 | 41,074 | 0.8485× | 0.86× | ✅ |
| pgsql | 23,752 | 24,594 | 0.9658× | 0.97× | ✅ |
| sqlite | 24,056 | 25,232 | 0.9534× | 0.96× | ✅ |
| curl | 86,415 | 90,448 | 0.9554× | 0.96× | ✅ |
| dns | 231,885 | 233,250 | 0.9941× | 1.0× | ✅ |

Geometric mean 0.9196×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
