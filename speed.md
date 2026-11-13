# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2632798` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,950 | 15,210 | 0.9171× | 0.92× | ✅ |
| echo-1k | 17,074 | 18,162 | 0.9401× | 0.94× | ✅ |
| json | 22,070 | 23,161 | 0.9529× | 0.95× | ✅ |
| service-health | 16,571 | 17,773 | 0.9324× | 0.93× | ✅ |
| websocket | 6,560 | 8,191 | 0.8009× | 0.81× | ✅ |
| timer | 203,133 | 313,646 | 0.6476× | 0.65× | ✅ |
| app | 125,427 | 136,001 | 0.9223× | 0.93× | ✅ |
| http2 | 18,613 | 20,463 | 0.9096× | 0.92× | ✅ |
| table | 195,546 | 201,865 | 0.9687× | 0.97× | ✅ |
| channel | 137,483 | 154,651 | 0.8890× | 0.89× | ✅ |
| task | 32,019 | 33,268 | 0.9624× | 0.96× | ✅ |
| mysql | 49,538 | 51,064 | 0.9701× | 0.97× | ✅ |
| http-client | 33,771 | 35,808 | 0.9431× | 0.94× | ✅ |
| client | 18,524 | 19,758 | 0.9375× | 0.94× | ✅ |
| socket | 19,651 | 21,067 | 0.9328× | 0.92× | ✅ |
| gzip | 194,154 | 197,083 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,394 | 181,572 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,774 | 4,762 | 1.0026× | 1.01× | ➖ |
| tcp-eof | 5,297 | 10,231 | 0.5178× | 0.52× | ✅ |
| udp | 7,564 | 7,986 | 0.9471× | 0.95× | ✅ |
| connect | 18,066 | 18,707 | 0.9657× | 0.98× | ✅ |
| https | 108,710 | 108,810 | 0.9991× | 1.0× | ✅ |
| wss | 104,291 | 104,800 | 0.9951× | 1.0× | ✅ |
| atomic | 49,752 | 51,316 | 0.9695× | 0.97× | ✅ |
| process-mode | 15,751 | 16,959 | 0.9288× | 0.94× | ✅ |
| http2-concurrent | 357,515 | 408,881 | 0.8744× | 0.88× | ✅ |
| websocket-64k | 156,005 | 159,695 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,793 | 16,327 | 0.7835× | 0.79× | ✅ |
| upload | 477,929 | 1,322,381 | 0.3614× | 0.37× | ✅ |
| static | 28,790 | 41,045 | 0.7014× | 0.72× | ✅ |
| pgsql | 23,276 | 24,595 | 0.9463× | 0.95× | ✅ |
| sqlite | 23,526 | 25,221 | 0.9328× | 0.94× | ✅ |
| curl | 85,773 | 90,363 | 0.9492× | 0.94× | ✅ |
| dns | 230,838 | 233,101 | 0.9903× | 0.99× | ✅ |

Geometric mean 0.8798×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
