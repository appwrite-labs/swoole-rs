# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4b5c892` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,923 | 15,214 | 0.9151× | 0.91× | ✅ |
| echo-1k | 16,951 | 18,162 | 0.9333× | 0.93× | ✅ |
| json | 22,056 | 23,161 | 0.9523× | 0.95× | ✅ |
| service-health | 16,499 | 17,777 | 0.9281× | 0.93× | ✅ |
| websocket | 6,569 | 8,188 | 0.8022× | 0.8× | ✅ |
| timer | 207,065 | 314,898 | 0.6576× | 0.65× | ✅ |
| app | 126,370 | 135,978 | 0.9293× | 0.93× | ✅ |
| http2 | 19,017 | 20,467 | 0.9291× | 0.92× | ✅ |
| table | 195,164 | 201,865 | 0.9668× | 0.97× | ✅ |
| channel | 137,540 | 155,170 | 0.8864× | 0.88× | ✅ |
| task | 32,477 | 33,257 | 0.9765× | 0.96× | ✅ |
| mysql | 49,692 | 51,061 | 0.9732× | 0.97× | ✅ |
| http-client | 33,551 | 35,807 | 0.9370× | 0.93× | ✅ |
| gzip | 194,025 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 178,967 | 181,568 | 0.9857× | 0.99× | ✅ |
| tcp-length | 4,643 | 4,762 | 0.9750× | 0.98× | ✅ |
| tcp-eof | 5,203 | 10,231 | 0.5086× | 0.51× | ✅ |
| udp | 7,541 | 7,986 | 0.9442× | 0.93× | ✅ |
| connect | 17,848 | 18,707 | 0.9541× | 0.94× | ✅ |
| https | 108,124 | 108,803 | 0.9938× | 1.0× | ✅ |
| wss | 103,678 | 104,800 | 0.9893× | 0.99× | ✅ |
| atomic | 50,015 | 51,316 | 0.9746× | 0.97× | ✅ |
| process-mode | 15,088 | 16,960 | 0.8896× | 0.88× | ✅ |
| http2-concurrent | 361,658 | 409,069 | 0.8841× | 0.88× | ✅ |
| websocket-64k | 89,869 | 159,691 | 0.5628× | 0.57× | ✅ |
| websocket-fragmented | 11,970 | 16,327 | 0.7331× | 0.72× | ✅ |
| upload | 414,007 | 1,322,388 | 0.3131× | 0.32× | ✅ |
| static | 27,036 | 40,993 | 0.6595× | 0.66× | ✅ |
| client | 18,595 | 19,757 | 0.9412× | 0.94× | ✅ |
| socket | 19,754 | 21,073 | 0.9374× | 0.92× | ✅ |
| pgsql | 23,487 | 24,594 | 0.9550× | 0.95× | ✅ |
| sqlite | 23,604 | 25,236 | 0.9353× | 0.93× | ✅ |
| curl | 85,870 | 90,261 | 0.9514× | 0.94× | ✅ |
| dns | 229,642 | 232,833 | 0.9863× | 0.99× | ✅ |

Geometric mean 0.8576×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
