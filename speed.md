# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7c3a169` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,943 | 15,213 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,026 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,524 | 17,777 | 0.9295× | 0.93× | ✅ |
| websocket | 6,584 | 8,188 | 0.8042× | 0.81× | ✅ |
| timer | 203,401 | 313,553 | 0.6487× | 0.65× | ✅ |
| app | 125,349 | 135,833 | 0.9228× | 0.93× | ✅ |
| http2 | 18,655 | 20,468 | 0.9115× | 0.92× | ✅ |
| table | 195,481 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,419 | 154,654 | 0.8886× | 0.9× | ✅ |
| task | 31,986 | 33,261 | 0.9617× | 0.96× | ✅ |
| mysql | 49,499 | 51,061 | 0.9694× | 0.97× | ✅ |
| http-client | 33,643 | 35,798 | 0.9398× | 0.94× | ✅ |
| client | 18,439 | 19,756 | 0.9333× | 0.94× | ✅ |
| socket | 19,563 | 21,067 | 0.9286× | 0.92× | ✅ |
| gzip | 194,148 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,391 | 181,572 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.01× | ➖ |
| tcp-eof | 5,307 | 10,231 | 0.5187× | 0.53× | ✅ |
| udp | 7,575 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,378 | 18,707 | 0.9824× | 0.98× | ✅ |
| https | 108,788 | 108,803 | 0.9999× | 1.0× | ✅ |
| wss | 104,405 | 104,794 | 0.9963× | 1.0× | ✅ |
| atomic | 49,688 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,687 | 16,963 | 0.9837× | 0.99× | ✅ |
| http2-concurrent | 359,859 | 408,877 | 0.8801× | 0.88× | ✅ |
| websocket-64k | 156,027 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,331 | 0.7847× | 0.79× | ✅ |
| upload | 478,071 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,388 | 41,103 | 0.7150× | 0.72× | ✅ |
| pgsql | 23,216 | 24,596 | 0.9439× | 0.95× | ✅ |
| sqlite | 23,483 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,861 | 90,483 | 0.9379× | 0.94× | ✅ |
| dns | 230,972 | 233,188 | 0.9905× | 0.99× | ✅ |

Geometric mean 0.8819×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
