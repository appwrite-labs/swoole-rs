# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,214 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,567 | 17,777 | 0.9319× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 205,734 | 313,543 | 0.6562× | 0.65× | ✅ |
| app | 125,375 | 135,940 | 0.9223× | 0.93× | ✅ |
| http2 | 18,676 | 20,465 | 0.9126× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,087 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,506 | 33,264 | 0.9772× | 0.96× | ✅ |
| mysql | 49,552 | 51,064 | 0.9704× | 0.97× | ✅ |
| http-client | 33,815 | 35,804 | 0.9444× | 0.94× | ✅ |
| client | 18,553 | 19,757 | 0.9390× | 0.94× | ✅ |
| socket | 19,677 | 21,073 | 0.9337× | 0.92× | ✅ |
| gzip | 194,170 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9445× | 0.93× | ✅ |
| connect | 17,741 | 18,707 | 0.9484× | 0.94× | ✅ |
| https | 108,199 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,849 | 104,800 | 0.9909× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,098 | 16,959 | 0.8902× | 0.88× | ✅ |
| http2-concurrent | 362,205 | 409,073 | 0.8854× | 0.88× | ✅ |
| websocket-64k | 89,956 | 159,695 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,328 | 0.7270× | 0.73× | ✅ |
| upload | 413,458 | 1,322,388 | 0.3127× | 0.32× | ✅ |
| static | 27,313 | 41,051 | 0.6653× | 0.66× | ✅ |
| pgsql | 23,374 | 24,593 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,544 | 25,236 | 0.9329× | 0.93× | ✅ |
| curl | 86,052 | 90,355 | 0.9524× | 0.94× | ✅ |
| dns | 231,599 | 232,923 | 0.9943× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
