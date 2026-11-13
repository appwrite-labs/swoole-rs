# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8d3e0ab` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,949 | 15,210 | 0.9171× | 0.92× | ✅ |
| echo-1k | 17,083 | 18,166 | 0.9404× | 0.94× | ✅ |
| json | 22,057 | 23,161 | 0.9523× | 0.95× | ✅ |
| service-health | 16,559 | 17,777 | 0.9315× | 0.93× | ✅ |
| websocket | 6,560 | 8,184 | 0.8015× | 0.81× | ✅ |
| timer | 203,082 | 313,568 | 0.6476× | 0.65× | ✅ |
| app | 125,432 | 135,881 | 0.9231× | 0.93× | ✅ |
| http2 | 18,612 | 20,468 | 0.9093× | 0.92× | ✅ |
| table | 195,533 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,478 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,027 | 33,264 | 0.9628× | 0.96× | ✅ |
| mysql | 49,568 | 51,062 | 0.9707× | 0.97× | ✅ |
| http-client | 33,730 | 35,809 | 0.9420× | 0.94× | ✅ |
| client | 18,515 | 19,756 | 0.9371× | 0.94× | ✅ |
| socket | 19,642 | 21,067 | 0.9323× | 0.92× | ✅ |
| gzip | 194,151 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,346 | 181,568 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,774 | 4,762 | 1.0026× | 1.01× | ➖ |
| tcp-eof | 5,297 | 10,231 | 0.5178× | 0.52× | ✅ |
| udp | 7,572 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,068 | 18,707 | 0.9658× | 0.98× | ✅ |
| https | 108,818 | 108,803 | 1.0001× | 1.0× | ❌ |
| wss | 104,376 | 104,800 | 0.9960× | 1.0× | ✅ |
| atomic | 49,740 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,812 | 16,960 | 0.9323× | 0.94× | ✅ |
| http2-concurrent | 357,386 | 408,877 | 0.8741× | 0.88× | ✅ |
| websocket-64k | 156,001 | 159,692 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,789 | 16,331 | 0.7831× | 0.79× | ✅ |
| upload | 477,913 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 28,938 | 41,103 | 0.7040× | 0.72× | ✅ |
| pgsql | 23,264 | 24,596 | 0.9459× | 0.95× | ✅ |
| sqlite | 23,533 | 25,236 | 0.9325× | 0.94× | ✅ |
| curl | 85,772 | 90,354 | 0.9493× | 0.94× | ✅ |
| dns | 231,169 | 233,064 | 0.9919× | 0.99× | ✅ |

Geometric mean 0.8801×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
