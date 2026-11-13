# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c29c6cb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,946 | 15,214 | 0.9167× | 0.92× | ✅ |
| echo-1k | 17,083 | 18,162 | 0.9406× | 0.94× | ✅ |
| json | 22,061 | 23,158 | 0.9526× | 0.95× | ✅ |
| service-health | 16,562 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,564 | 8,188 | 0.8016× | 0.81× | ✅ |
| timer | 203,142 | 313,553 | 0.6479× | 0.65× | ✅ |
| app | 125,376 | 135,878 | 0.9227× | 0.93× | ✅ |
| http2 | 18,620 | 20,468 | 0.9097× | 0.92× | ✅ |
| table | 195,537 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,474 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,030 | 33,264 | 0.9629× | 0.96× | ✅ |
| mysql | 49,566 | 51,052 | 0.9709× | 0.97× | ✅ |
| http-client | 33,731 | 35,806 | 0.9420× | 0.94× | ✅ |
| client | 18,515 | 19,757 | 0.9371× | 0.94× | ✅ |
| socket | 19,642 | 21,067 | 0.9323× | 0.92× | ✅ |
| gzip | 194,154 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,346 | 181,568 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,774 | 4,762 | 1.0026× | 1.01× | ➖ |
| tcp-eof | 5,297 | 10,231 | 0.5178× | 0.52× | ✅ |
| udp | 7,573 | 7,986 | 0.9483× | 0.95× | ✅ |
| connect | 18,068 | 18,707 | 0.9659× | 0.98× | ✅ |
| https | 108,804 | 108,803 | 1.0000× | 1.0× | ❌ |
| wss | 104,383 | 104,794 | 0.9961× | 1.0× | ✅ |
| atomic | 49,743 | 51,316 | 0.9694× | 0.97× | ✅ |
| process-mode | 15,813 | 16,956 | 0.9326× | 0.94× | ✅ |
| http2-concurrent | 357,391 | 408,882 | 0.8741× | 0.88× | ✅ |
| websocket-64k | 156,007 | 159,696 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,793 | 16,327 | 0.7835× | 0.79× | ✅ |
| upload | 477,907 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 28,921 | 41,074 | 0.7041× | 0.72× | ✅ |
| pgsql | 23,263 | 24,595 | 0.9459× | 0.95× | ✅ |
| sqlite | 23,533 | 25,236 | 0.9325× | 0.94× | ✅ |
| curl | 85,771 | 90,460 | 0.9482× | 0.94× | ✅ |
| dns | 230,452 | 232,615 | 0.9907× | 0.99× | ✅ |

Geometric mean 0.8801×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
