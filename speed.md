# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4de9054` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,950 | 15,214 | 0.9169× | 0.92× | ✅ |
| echo-1k | 17,079 | 18,165 | 0.9402× | 0.94× | ✅ |
| json | 22,060 | 23,158 | 0.9526× | 0.95× | ✅ |
| service-health | 16,562 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,564 | 8,188 | 0.8016× | 0.81× | ✅ |
| timer | 203,171 | 313,585 | 0.6479× | 0.65× | ✅ |
| app | 125,388 | 135,975 | 0.9221× | 0.93× | ✅ |
| http2 | 18,618 | 20,468 | 0.9096× | 0.92× | ✅ |
| table | 195,537 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,474 | 154,651 | 0.8889× | 0.89× | ✅ |
| task | 32,031 | 33,261 | 0.9630× | 0.96× | ✅ |
| mysql | 49,564 | 51,017 | 0.9715× | 0.97× | ✅ |
| http-client | 33,735 | 35,809 | 0.9421× | 0.94× | ✅ |
| client | 18,514 | 19,757 | 0.9371× | 0.94× | ✅ |
| socket | 19,642 | 21,067 | 0.9323× | 0.92× | ✅ |
| gzip | 194,151 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,391 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,774 | 4,762 | 1.0026× | 1.01× | ➖ |
| tcp-eof | 5,297 | 10,231 | 0.5178× | 0.52× | ✅ |
| udp | 7,572 | 7,986 | 0.9481× | 0.95× | ✅ |
| connect | 18,071 | 18,707 | 0.9660× | 0.98× | ✅ |
| https | 108,819 | 108,810 | 1.0001× | 1.0× | ❌ |
| wss | 104,379 | 104,800 | 0.9960× | 1.0× | ✅ |
| atomic | 49,743 | 51,316 | 0.9694× | 0.97× | ✅ |
| process-mode | 15,809 | 16,959 | 0.9322× | 0.94× | ✅ |
| http2-concurrent | 357,637 | 408,882 | 0.8747× | 0.88× | ✅ |
| websocket-64k | 156,008 | 159,696 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,788 | 16,328 | 0.7832× | 0.79× | ✅ |
| upload | 477,910 | 1,322,384 | 0.3614× | 0.37× | ✅ |
| static | 28,892 | 41,045 | 0.7039× | 0.72× | ✅ |
| pgsql | 23,264 | 24,595 | 0.9459× | 0.95× | ✅ |
| sqlite | 23,530 | 25,236 | 0.9324× | 0.94× | ✅ |
| curl | 85,777 | 90,545 | 0.9473× | 0.94× | ✅ |
| dns | 230,784 | 232,911 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8800×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
