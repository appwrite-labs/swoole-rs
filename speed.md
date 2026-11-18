# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c3e7e3d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,746 | 15,213 | 0.9035× | 0.91× | ✅ |
| echo-1k | 16,742 | 18,165 | 0.9216× | 0.92× | ✅ |
| json | 22,070 | 23,164 | 0.9528× | 0.95× | ✅ |
| service-health | 16,317 | 17,777 | 0.9179× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8153× | 0.81× | ✅ |
| timer | 201,541 | 313,788 | 0.6423× | 0.65× | ✅ |
| app | 127,006 | 135,876 | 0.9347× | 0.93× | ✅ |
| http2 | 18,975 | 20,497 | 0.9258× | 0.92× | ✅ |
| table | 193,458 | 202,900 | 0.9535× | 0.96× | ✅ |
| channel | 137,972 | 155,714 | 0.8861× | 0.88× | ✅ |
| task | 31,949 | 33,265 | 0.9604× | 0.96× | ✅ |
| mysql | 50,064 | 51,088 | 0.9799× | 0.97× | ✅ |
| http-client | 33,404 | 35,793 | 0.9333× | 0.93× | ✅ |
| gzip | 194,360 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.5× | ✅ |
| udp | 7,580 | 7,987 | 0.9490× | 0.95× | ✅ |
| connect | 17,580 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,026 | 108,813 | 0.9744× | 0.98× | ✅ |
| wss | 101,240 | 104,804 | 0.9660× | 0.97× | ✅ |
| atomic | 50,176 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,267 | 17,024 | 0.8968× | 0.89× | ✅ |
| http2-concurrent | 364,760 | 409,053 | 0.8917× | 0.89× | ✅ |
| websocket-64k | 89,981 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,425 | 1,322,427 | 0.3149× | 0.32× | ✅ |
| static | 27,076 | 41,028 | 0.6599× | 0.65× | ✅ |
| client | 18,477 | 19,729 | 0.9365× | 0.93× | ✅ |
| socket | 19,700 | 21,045 | 0.9361× | 0.93× | ✅ |
| pgsql | 22,929 | 24,619 | 0.9313× | 0.92× | ✅ |
| sqlite | 23,426 | 25,236 | 0.9283× | 0.93× | ✅ |
| curl | 85,188 | 90,359 | 0.9428× | 0.95× | ✅ |
| dns | 229,848 | 232,526 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
