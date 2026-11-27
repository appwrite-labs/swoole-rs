# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,050 | 23,164 | 0.9519× | 0.95× | ✅ |
| service-health | 16,308 | 17,777 | 0.9174× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,071 | 313,793 | 0.6440× | 0.65× | ✅ |
| app | 127,537 | 135,959 | 0.9381× | 0.93× | ✅ |
| http2 | 19,182 | 20,500 | 0.9357× | 0.92× | ✅ |
| table | 194,341 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,268 | 0.9641× | 0.96× | ✅ |
| mysql | 50,262 | 51,088 | 0.9838× | 0.97× | ✅ |
| http-client | 33,651 | 35,793 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,570 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,403 | 17,021 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 366,964 | 409,058 | 0.8971× | 0.89× | ✅ |
| websocket-64k | 90,041 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,907 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,426 | 41,092 | 0.6188× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,888 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,474 | 25,268 | 0.9290× | 0.93× | ✅ |
| curl | 86,078 | 90,418 | 0.9520× | 0.95× | ✅ |
| dns | 230,400 | 233,401 | 0.9871× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
