# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4d4694d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,213 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,051 | 23,164 | 0.9519× | 0.95× | ✅ |
| service-health | 16,308 | 17,777 | 0.9174× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,100 | 313,758 | 0.6441× | 0.65× | ✅ |
| app | 127,583 | 136,022 | 0.9380× | 0.93× | ✅ |
| http2 | 19,180 | 20,500 | 0.9356× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,275 | 51,088 | 0.9841× | 0.97× | ✅ |
| http-client | 33,653 | 35,793 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,571 | 104,803 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,021 | 0.9050× | 0.89× | ✅ |
| http2-concurrent | 367,085 | 409,058 | 0.8974× | 0.89× | ✅ |
| websocket-64k | 90,041 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,896 | 1,322,433 | 0.3168× | 0.32× | ✅ |
| static | 25,370 | 41,005 | 0.6187× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,836 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,886 | 24,662 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,076 | 90,523 | 0.9509× | 0.95× | ✅ |
| dns | 230,763 | 233,082 | 0.9900× | 0.99× | ✅ |
| file | 108,880 | 131,296 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
