# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `976d251` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,856 | 15,210 | 0.9110× | 0.91× | ✅ |
| echo-1k | 16,889 | 18,165 | 0.9298× | 0.93× | ✅ |
| json | 21,987 | 23,162 | 0.9493× | 0.95× | ✅ |
| service-health | 16,436 | 17,773 | 0.9248× | 0.93× | ✅ |
| websocket | 6,540 | 8,188 | 0.7987× | 0.8× | ✅ |
| timer | 206,023 | 313,665 | 0.6568× | 0.65× | ✅ |
| app | 125,570 | 135,948 | 0.9237× | 0.93× | ✅ |
| http2 | 18,948 | 20,468 | 0.9258× | 0.91× | ✅ |
| table | 195,094 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 136,801 | 155,170 | 0.8816× | 0.88× | ✅ |
| task | 32,400 | 33,268 | 0.9739× | 0.96× | ✅ |
| mysql | 49,602 | 50,985 | 0.9729× | 0.97× | ✅ |
| http-client | 33,469 | 35,803 | 0.9348× | 0.93× | ✅ |
| gzip | 193,964 | 197,079 | 0.9842× | 0.99× | ✅ |
| websocket-deflate | 178,853 | 181,568 | 0.9850× | 0.99× | ✅ |
| tcp-length | 4,633 | 4,765 | 0.9722× | 0.98× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.51× | ✅ |
| udp | 7,518 | 7,986 | 0.9414× | 0.93× | ✅ |
| connect | 17,637 | 18,703 | 0.9430× | 0.94× | ✅ |
| https | 108,131 | 108,803 | 0.9938× | 1.0× | ✅ |
| wss | 103,707 | 104,797 | 0.9896× | 0.99× | ✅ |
| atomic | 49,952 | 51,316 | 0.9734× | 0.97× | ✅ |
| process-mode | 15,041 | 16,956 | 0.8871× | 0.88× | ✅ |
| http2-concurrent | 360,394 | 409,074 | 0.8810× | 0.88× | ✅ |
| websocket-64k | 89,849 | 159,697 | 0.5626× | 0.57× | ✅ |
| websocket-fragmented | 11,936 | 16,331 | 0.7309× | 0.72× | ✅ |
| upload | 413,513 | 1,322,387 | 0.3127× | 0.32× | ✅ |
| static | 26,938 | 41,051 | 0.6562× | 0.66× | ✅ |
| client | 18,523 | 19,757 | 0.9376× | 0.94× | ✅ |
| socket | 19,680 | 21,072 | 0.9339× | 0.92× | ✅ |
| pgsql | 23,275 | 24,595 | 0.9463× | 0.95× | ✅ |
| sqlite | 23,500 | 25,236 | 0.9312× | 0.93× | ✅ |
| curl | 85,985 | 90,353 | 0.9517× | 0.94× | ✅ |
| dns | 230,392 | 232,623 | 0.9904× | 0.99× | ✅ |

Geometric mean 0.8552×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
