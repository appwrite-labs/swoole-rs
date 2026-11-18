# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f1224ef` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,210 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,162 | 0.9210× | 0.92× | ✅ |
| json | 22,062 | 23,173 | 0.9520× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,676 | 8,192 | 0.8149× | 0.8× | ✅ |
| timer | 201,367 | 313,734 | 0.6418× | 0.65× | ✅ |
| app | 126,783 | 135,967 | 0.9324× | 0.93× | ✅ |
| http2 | 18,963 | 20,501 | 0.9250× | 0.92× | ✅ |
| table | 193,894 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 138,479 | 155,726 | 0.8892× | 0.88× | ✅ |
| task | 31,932 | 33,272 | 0.9597× | 0.96× | ✅ |
| mysql | 49,833 | 51,064 | 0.9759× | 0.97× | ✅ |
| http-client | 33,656 | 35,796 | 0.9402× | 0.93× | ✅ |
| gzip | 194,350 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,583 | 7,986 | 0.9495× | 0.95× | ✅ |
| connect | 17,572 | 18,730 | 0.9382× | 0.93× | ✅ |
| https | 106,028 | 108,813 | 0.9744× | 0.98× | ✅ |
| wss | 101,239 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,164 | 51,285 | 0.9781× | 0.97× | ✅ |
| process-mode | 15,255 | 17,025 | 0.8960× | 0.89× | ✅ |
| http2-concurrent | 364,729 | 409,056 | 0.8916× | 0.89× | ✅ |
| websocket-64k | 89,980 | 159,694 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,844 | 16,372 | 0.7234× | 0.72× | ✅ |
| upload | 416,224 | 1,322,429 | 0.3147× | 0.32× | ✅ |
| static | 27,103 | 41,070 | 0.6599× | 0.65× | ✅ |
| client | 18,378 | 19,727 | 0.9316× | 0.93× | ✅ |
| socket | 19,632 | 21,045 | 0.9328× | 0.93× | ✅ |
| pgsql | 22,586 | 24,617 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,341 | 90,356 | 0.9445× | 0.95× | ✅ |
| dns | 229,864 | 232,439 | 0.9889× | 0.99× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
