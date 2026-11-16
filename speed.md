# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7651513` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,853 | 15,214 | 0.9106× | 0.91× | ✅ |
| echo-1k | 16,886 | 18,165 | 0.9296× | 0.93× | ✅ |
| json | 21,994 | 23,161 | 0.9496× | 0.95× | ✅ |
| service-health | 16,439 | 17,777 | 0.9248× | 0.93× | ✅ |
| websocket | 6,541 | 8,188 | 0.7989× | 0.8× | ✅ |
| timer | 205,997 | 313,522 | 0.6570× | 0.65× | ✅ |
| app | 125,609 | 135,945 | 0.9240× | 0.93× | ✅ |
| http2 | 18,960 | 20,468 | 0.9263× | 0.91× | ✅ |
| table | 195,105 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 136,804 | 155,170 | 0.8816× | 0.88× | ✅ |
| task | 32,403 | 33,263 | 0.9741× | 0.96× | ✅ |
| mysql | 49,573 | 51,061 | 0.9709× | 0.97× | ✅ |
| http-client | 33,479 | 35,803 | 0.9351× | 0.93× | ✅ |
| gzip | 193,956 | 197,079 | 0.9842× | 0.99× | ✅ |
| websocket-deflate | 178,856 | 181,572 | 0.9850× | 0.99× | ✅ |
| tcp-length | 4,633 | 4,762 | 0.9729× | 0.98× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.51× | ✅ |
| udp | 7,518 | 7,986 | 0.9414× | 0.93× | ✅ |
| connect | 17,637 | 18,707 | 0.9428× | 0.94× | ✅ |
| https | 108,120 | 108,806 | 0.9937× | 1.0× | ✅ |
| wss | 103,710 | 104,794 | 0.9897× | 0.99× | ✅ |
| atomic | 49,959 | 51,312 | 0.9736× | 0.97× | ✅ |
| process-mode | 15,049 | 16,959 | 0.8874× | 0.88× | ✅ |
| http2-concurrent | 360,334 | 409,069 | 0.8809× | 0.88× | ✅ |
| websocket-64k | 89,847 | 159,693 | 0.5626× | 0.57× | ✅ |
| websocket-fragmented | 11,932 | 16,327 | 0.7308× | 0.72× | ✅ |
| upload | 413,917 | 1,322,390 | 0.3130× | 0.32× | ✅ |
| static | 26,938 | 41,051 | 0.6562× | 0.66× | ✅ |
| client | 18,527 | 19,757 | 0.9378× | 0.94× | ✅ |
| socket | 19,688 | 21,072 | 0.9343× | 0.92× | ✅ |
| pgsql | 23,268 | 24,594 | 0.9461× | 0.95× | ✅ |
| sqlite | 23,496 | 25,236 | 0.9311× | 0.93× | ✅ |
| curl | 85,981 | 90,350 | 0.9516× | 0.94× | ✅ |
| dns | 230,834 | 232,800 | 0.9916× | 0.99× | ✅ |

Geometric mean 0.8553×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
