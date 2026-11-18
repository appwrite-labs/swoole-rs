# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f1224ef` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,733 | 15,213 | 0.9027× | 0.91× | ✅ |
| echo-1k | 16,724 | 18,165 | 0.9207× | 0.92× | ✅ |
| json | 22,069 | 23,164 | 0.9527× | 0.95× | ✅ |
| service-health | 16,305 | 17,777 | 0.9172× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8153× | 0.8× | ✅ |
| timer | 201,515 | 313,724 | 0.6423× | 0.65× | ✅ |
| app | 126,779 | 135,939 | 0.9326× | 0.93× | ✅ |
| http2 | 18,967 | 20,497 | 0.9253× | 0.92× | ✅ |
| table | 193,449 | 202,900 | 0.9534× | 0.96× | ✅ |
| channel | 137,963 | 155,714 | 0.8860× | 0.88× | ✅ |
| task | 31,935 | 33,265 | 0.9600× | 0.96× | ✅ |
| mysql | 49,837 | 51,064 | 0.9760× | 0.97× | ✅ |
| http-client | 33,653 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,346 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,584 | 7,987 | 0.9495× | 0.95× | ✅ |
| connect | 17,569 | 18,734 | 0.9378× | 0.93× | ✅ |
| https | 106,024 | 108,802 | 0.9745× | 0.98× | ✅ |
| wss | 101,239 | 104,800 | 0.9660× | 0.97× | ✅ |
| atomic | 50,167 | 51,285 | 0.9782× | 0.97× | ✅ |
| process-mode | 15,254 | 17,024 | 0.8960× | 0.89× | ✅ |
| http2-concurrent | 364,746 | 409,057 | 0.8917× | 0.89× | ✅ |
| websocket-64k | 89,980 | 159,695 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,223 | 1,322,430 | 0.3147× | 0.32× | ✅ |
| static | 27,132 | 41,086 | 0.6604× | 0.65× | ✅ |
| client | 18,378 | 19,730 | 0.9315× | 0.93× | ✅ |
| socket | 19,631 | 21,046 | 0.9328× | 0.93× | ✅ |
| pgsql | 22,586 | 24,617 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,347 | 90,322 | 0.9449× | 0.95× | ✅ |
| dns | 229,842 | 233,077 | 0.9861× | 0.99× | ✅ |

Geometric mean 0.8530×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
