# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `61eaa01` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,817 | 15,213 | 0.9082× | 0.91× | ✅ |
| echo-1k | 16,825 | 18,165 | 0.9262× | 0.93× | ✅ |
| json | 21,977 | 23,161 | 0.9489× | 0.95× | ✅ |
| service-health | 16,381 | 17,777 | 0.9215× | 0.92× | ✅ |
| websocket | 6,612 | 8,188 | 0.8076× | 0.8× | ✅ |
| timer | 205,469 | 313,626 | 0.6551× | 0.65× | ✅ |
| app | 126,736 | 135,950 | 0.9322× | 0.93× | ✅ |
| http2 | 18,967 | 20,469 | 0.9266× | 0.92× | ✅ |
| table | 194,415 | 201,865 | 0.9631× | 0.97× | ✅ |
| channel | 137,352 | 155,174 | 0.8851× | 0.88× | ✅ |
| task | 32,475 | 33,264 | 0.9763× | 0.96× | ✅ |
| mysql | 49,923 | 51,052 | 0.9779× | 0.97× | ✅ |
| http-client | 33,684 | 35,804 | 0.9408× | 0.93× | ✅ |
| gzip | 194,487 | 197,079 | 0.9868× | 0.99× | ✅ |
| websocket-deflate | 179,018 | 181,568 | 0.9860× | 0.99× | ✅ |
| tcp-length | 4,583 | 4,762 | 0.9624× | 0.96× | ✅ |
| tcp-eof | 5,163 | 10,231 | 0.5046× | 0.5× | ✅ |
| udp | 7,488 | 7,986 | 0.9376× | 0.93× | ✅ |
| connect | 17,507 | 18,707 | 0.9358× | 0.93× | ✅ |
| https | 105,990 | 108,803 | 0.9742× | 0.98× | ✅ |
| wss | 101,045 | 104,794 | 0.9642× | 0.97× | ✅ |
| atomic | 49,906 | 51,316 | 0.9725× | 0.97× | ✅ |
| process-mode | 15,077 | 16,962 | 0.8889× | 0.88× | ✅ |
| http2-concurrent | 364,275 | 409,072 | 0.8905× | 0.88× | ✅ |
| websocket-64k | 89,867 | 159,697 | 0.5627× | 0.57× | ✅ |
| websocket-fragmented | 11,713 | 16,327 | 0.7174× | 0.72× | ✅ |
| upload | 415,978 | 1,322,388 | 0.3146× | 0.32× | ✅ |
| static | 26,690 | 41,051 | 0.6502× | 0.65× | ✅ |
| client | 18,559 | 19,756 | 0.9394× | 0.94× | ✅ |
| socket | 19,738 | 21,072 | 0.9367× | 0.92× | ✅ |
| pgsql | 22,798 | 24,595 | 0.9270× | 0.93× | ✅ |
| sqlite | 23,505 | 25,236 | 0.9314× | 0.93× | ✅ |
| curl | 86,475 | 90,447 | 0.9561× | 0.94× | ✅ |
| dns | 230,298 | 232,854 | 0.9890× | 0.99× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
