# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a726d2b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,924 | 15,213 | 0.9153× | 0.92× | ✅ |
| echo-1k | 17,025 | 18,162 | 0.9374× | 0.94× | ✅ |
| json | 22,011 | 23,161 | 0.9504× | 0.95× | ✅ |
| service-health | 16,513 | 17,773 | 0.9291× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.82× | ✅ |
| timer | 203,687 | 313,532 | 0.6497× | 0.65× | ✅ |
| app | 125,688 | 135,921 | 0.9247× | 0.93× | ✅ |
| http2 | 18,650 | 20,464 | 0.9114× | 0.93× | ✅ |
| table | 195,466 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,433 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,098 | 33,268 | 0.9648× | 0.96× | ✅ |
| mysql | 49,506 | 51,056 | 0.9696× | 0.97× | ✅ |
| http-client | 33,686 | 35,805 | 0.9408× | 0.94× | ✅ |
| client | 18,438 | 19,757 | 0.9332× | 0.94× | ✅ |
| socket | 19,578 | 21,067 | 0.9293× | 0.92× | ✅ |
| gzip | 194,120 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,397 | 181,568 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0063× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,987 | 0.9482× | 0.95× | ✅ |
| connect | 18,351 | 18,707 | 0.9810× | 0.98× | ✅ |
| https | 108,791 | 108,813 | 0.9998× | 1.0× | ✅ |
| wss | 104,477 | 104,800 | 0.9969× | 1.0× | ✅ |
| atomic | 49,673 | 51,316 | 0.9680× | 0.97× | ✅ |
| process-mode | 16,669 | 17,028 | 0.9789× | 0.99× | ✅ |
| http2-concurrent | 358,982 | 408,877 | 0.8780× | 0.88× | ✅ |
| websocket-64k | 156,026 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,825 | 16,331 | 0.7853× | 0.79× | ✅ |
| upload | 478,005 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,278 | 41,045 | 0.7133× | 0.72× | ✅ |
| pgsql | 23,197 | 24,595 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,486 | 25,236 | 0.9306× | 0.94× | ✅ |
| curl | 84,839 | 90,449 | 0.9380× | 0.94× | ✅ |
| dns | 230,589 | 232,356 | 0.9924× | 0.99× | ✅ |

Geometric mean 0.8821×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
