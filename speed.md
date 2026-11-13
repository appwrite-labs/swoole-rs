# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `eb9636e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,920 | 15,210 | 0.9152× | 0.92× | ✅ |
| echo-1k | 17,022 | 18,165 | 0.9370× | 0.94× | ✅ |
| json | 22,026 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,524 | 17,777 | 0.9295× | 0.93× | ✅ |
| websocket | 6,577 | 8,188 | 0.8033× | 0.82× | ✅ |
| timer | 203,271 | 313,672 | 0.6480× | 0.65× | ✅ |
| app | 125,781 | 135,918 | 0.9254× | 0.93× | ✅ |
| http2 | 18,650 | 20,468 | 0.9112× | 0.93× | ✅ |
| table | 195,477 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,413 | 154,654 | 0.8885× | 0.9× | ✅ |
| task | 32,098 | 33,265 | 0.9649× | 0.96× | ✅ |
| mysql | 49,573 | 51,064 | 0.9708× | 0.97× | ✅ |
| http-client | 33,753 | 35,808 | 0.9426× | 0.94× | ✅ |
| client | 18,483 | 19,757 | 0.9355× | 0.94× | ✅ |
| socket | 19,596 | 21,067 | 0.9302× | 0.92× | ✅ |
| gzip | 194,117 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,383 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,581 | 7,986 | 0.9492× | 0.95× | ✅ |
| connect | 18,331 | 18,707 | 0.9799× | 0.98× | ✅ |
| https | 108,810 | 108,803 | 1.0001× | 1.0× | ❌ |
| wss | 104,468 | 104,797 | 0.9969× | 1.0× | ✅ |
| atomic | 49,688 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,674 | 16,957 | 0.9833× | 0.99× | ✅ |
| http2-concurrent | 359,094 | 409,559 | 0.8768× | 0.88× | ✅ |
| websocket-64k | 156,019 | 159,699 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,331 | 0.7847× | 0.79× | ✅ |
| upload | 477,839 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 29,281 | 41,074 | 0.7129× | 0.72× | ✅ |
| pgsql | 23,200 | 24,596 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,482 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,745 | 90,483 | 0.9366× | 0.94× | ✅ |
| dns | 231,044 | 233,186 | 0.9908× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
