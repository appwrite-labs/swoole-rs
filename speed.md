# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `af96a17` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,920 | 15,210 | 0.9152× | 0.92× | ✅ |
| echo-1k | 17,022 | 18,165 | 0.9371× | 0.94× | ✅ |
| json | 22,026 | 23,162 | 0.9510× | 0.95× | ✅ |
| service-health | 16,524 | 17,777 | 0.9295× | 0.93× | ✅ |
| websocket | 6,581 | 8,188 | 0.8038× | 0.82× | ✅ |
| timer | 203,237 | 313,715 | 0.6478× | 0.65× | ✅ |
| app | 125,752 | 135,955 | 0.9249× | 0.93× | ✅ |
| http2 | 18,658 | 20,464 | 0.9117× | 0.93× | ✅ |
| table | 195,481 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,413 | 154,654 | 0.8885× | 0.9× | ✅ |
| task | 32,088 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 49,579 | 51,054 | 0.9711× | 0.97× | ✅ |
| http-client | 33,762 | 35,803 | 0.9430× | 0.94× | ✅ |
| client | 18,483 | 19,758 | 0.9355× | 0.94× | ✅ |
| socket | 19,600 | 21,067 | 0.9304× | 0.92× | ✅ |
| gzip | 194,121 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,383 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,228 | 0.5221× | 0.54× | ✅ |
| udp | 7,581 | 7,986 | 0.9493× | 0.95× | ✅ |
| connect | 18,331 | 18,707 | 0.9799× | 0.98× | ✅ |
| https | 108,814 | 108,803 | 1.0001× | 1.0× | ❌ |
| wss | 104,483 | 104,800 | 0.9970× | 1.0× | ✅ |
| atomic | 49,688 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,675 | 17,024 | 0.9795× | 0.99× | ✅ |
| http2-concurrent | 359,261 | 408,962 | 0.8785× | 0.88× | ✅ |
| websocket-64k | 156,025 | 159,692 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,331 | 0.7845× | 0.79× | ✅ |
| upload | 477,825 | 1,322,384 | 0.3613× | 0.37× | ✅ |
| static | 29,322 | 41,103 | 0.7134× | 0.72× | ✅ |
| pgsql | 23,213 | 24,594 | 0.9439× | 0.95× | ✅ |
| sqlite | 23,482 | 25,221 | 0.9310× | 0.94× | ✅ |
| curl | 84,751 | 90,272 | 0.9388× | 0.94× | ✅ |
| dns | 230,846 | 232,970 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8821×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
