# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b7feb50` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,745 | 15,210 | 0.9036× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,162 | 0.9210× | 0.92× | ✅ |
| json | 22,062 | 23,174 | 0.9520× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,676 | 8,192 | 0.8149× | 0.8× | ✅ |
| timer | 201,507 | 313,726 | 0.6423× | 0.65× | ✅ |
| app | 127,004 | 135,987 | 0.9339× | 0.93× | ✅ |
| http2 | 18,964 | 20,500 | 0.9251× | 0.92× | ✅ |
| table | 193,894 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 137,967 | 155,727 | 0.8860× | 0.88× | ✅ |
| task | 31,931 | 33,272 | 0.9597× | 0.96× | ✅ |
| mysql | 50,053 | 51,077 | 0.9799× | 0.97× | ✅ |
| http-client | 33,833 | 35,797 | 0.9451× | 0.93× | ✅ |
| gzip | 194,346 | 197,099 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,579 | 7,986 | 0.9491× | 0.95× | ✅ |
| connect | 17,587 | 18,734 | 0.9388× | 0.93× | ✅ |
| https | 106,025 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,167 | 51,276 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,270 | 17,028 | 0.8968× | 0.89× | ✅ |
| http2-concurrent | 363,811 | 408,861 | 0.8898× | 0.89× | ✅ |
| websocket-64k | 89,980 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,225 | 1,322,429 | 0.3147× | 0.32× | ✅ |
| static | 27,031 | 40,999 | 0.6593× | 0.65× | ✅ |
| client | 18,466 | 19,721 | 0.9364× | 0.93× | ✅ |
| socket | 19,695 | 21,036 | 0.9362× | 0.93× | ✅ |
| pgsql | 22,603 | 24,614 | 0.9183× | 0.92× | ✅ |
| sqlite | 23,408 | 25,236 | 0.9276× | 0.93× | ✅ |
| curl | 85,347 | 90,355 | 0.9446× | 0.95× | ✅ |
| dns | 229,908 | 232,798 | 0.9876× | 0.99× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
