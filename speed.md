# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4d4a5cf` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,733 | 15,213 | 0.9027× | 0.91× | ✅ |
| echo-1k | 16,724 | 18,165 | 0.9207× | 0.92× | ✅ |
| json | 22,077 | 23,173 | 0.9527× | 0.95× | ✅ |
| service-health | 16,313 | 17,786 | 0.9172× | 0.92× | ✅ |
| websocket | 6,676 | 8,185 | 0.8156× | 0.8× | ✅ |
| timer | 201,328 | 314,082 | 0.6410× | 0.65× | ✅ |
| app | 126,673 | 135,993 | 0.9315× | 0.93× | ✅ |
| http2 | 18,966 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 193,905 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 138,491 | 155,723 | 0.8893× | 0.88× | ✅ |
| task | 31,935 | 33,269 | 0.9599× | 0.96× | ✅ |
| mysql | 49,800 | 51,064 | 0.9752× | 0.97× | ✅ |
| http-client | 33,629 | 35,793 | 0.9396× | 0.93× | ✅ |
| gzip | 194,346 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,583 | 7,987 | 0.9495× | 0.95× | ✅ |
| connect | 17,569 | 18,734 | 0.9378× | 0.93× | ✅ |
| https | 106,031 | 108,809 | 0.9745× | 0.98× | ✅ |
| wss | 101,243 | 104,806 | 0.9660× | 0.97× | ✅ |
| atomic | 50,175 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,257 | 17,024 | 0.8962× | 0.89× | ✅ |
| http2-concurrent | 364,806 | 409,057 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 89,982 | 159,694 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,844 | 16,368 | 0.7236× | 0.72× | ✅ |
| upload | 416,242 | 1,322,425 | 0.3148× | 0.32× | ✅ |
| static | 27,086 | 41,034 | 0.6601× | 0.65× | ✅ |
| client | 18,365 | 19,730 | 0.9308× | 0.93× | ✅ |
| socket | 19,599 | 21,045 | 0.9313× | 0.93× | ✅ |
| pgsql | 22,587 | 24,617 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,401 | 25,221 | 0.9278× | 0.93× | ✅ |
| curl | 85,343 | 90,355 | 0.9445× | 0.95× | ✅ |
| dns | 229,490 | 232,997 | 0.9849× | 0.99× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
