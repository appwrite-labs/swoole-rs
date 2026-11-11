# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f359bf5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,457 | 15,210 | 0.9505× | 0.94× | ✅ |
| echo-1k | 17,585 | 18,162 | 0.9682× | 0.97× | ✅ |
| json | 22,569 | 23,167 | 0.9742× | 0.97× | ✅ |
| service-health | 17,066 | 17,786 | 0.9595× | 0.95× | ✅ |
| websocket | 6,913 | 8,188 | 0.8443× | 0.84× | ✅ |
| timer | 202,610 | 313,736 | 0.6458× | 0.65× | ✅ |
| app | 126,713 | 135,931 | 0.9322× | 0.94× | ✅ |
| http2 | 20,412 | 20,464 | 0.9975× | 0.99× | ✅ |
| table | 196,221 | 201,874 | 0.9720× | 0.98× | ✅ |
| channel | 139,766 | 154,667 | 0.9037× | 0.91× | ✅ |
| task | 32,422 | 33,268 | 0.9746× | 0.97× | ✅ |
| mysql | 50,086 | 51,059 | 0.9809× | 0.98× | ✅ |
| http-client | 34,307 | 35,825 | 0.9576× | 0.96× | ✅ |
| client | 18,977 | 19,763 | 0.9603× | 0.95× | ✅ |
| socket | 20,371 | 21,071 | 0.9668× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0039× | 1.0× | ❌ |
| connect | 18,909 | 18,707 | 1.0108× | 0.99× | ❌ |
| https | 109,655 | 108,814 | 1.0077× | 1.01× | ➖ |
| wss | 105,077 | 104,808 | 1.0026× | 1.0× | ❌ |
| atomic | 50,283 | 51,325 | 0.9797× | 0.98× | ✅ |
| process-mode | 16,755 | 16,963 | 0.9878× | 0.98× | ✅ |
| http2-concurrent | 378,554 | 409,074 | 0.9254× | 0.92× | ✅ |
| websocket-64k | 156,359 | 159,706 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,883 | 16,336 | 0.7887× | 0.79× | ✅ |
| upload | 675,606 | 1,322,384 | 0.5109× | 0.51× | ✅ |
| static | 34,747 | 40,987 | 0.8478× | 0.85× | ✅ |
| pgsql | 23,753 | 24,594 | 0.9658× | 0.97× | ✅ |
| sqlite | 24,056 | 25,232 | 0.9534× | 0.96× | ✅ |
| curl | 86,080 | 90,571 | 0.9504× | 0.96× | ✅ |
| dns | 231,793 | 232,737 | 0.9959× | 1.0× | ✅ |

Geometric mean 0.9195×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
