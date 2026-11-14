# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `45bc2e2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,918 | 15,213 | 0.9148× | 0.92× | ✅ |
| echo-1k | 17,037 | 18,162 | 0.9381× | 0.94× | ✅ |
| json | 22,120 | 23,158 | 0.9552× | 0.95× | ✅ |
| service-health | 16,556 | 17,777 | 0.9314× | 0.93× | ✅ |
| websocket | 6,556 | 8,188 | 0.8008× | 0.8× | ✅ |
| timer | 205,902 | 313,463 | 0.6569× | 0.65× | ✅ |
| app | 125,599 | 135,853 | 0.9245× | 0.93× | ✅ |
| http2 | 18,635 | 20,464 | 0.9106× | 0.91× | ✅ |
| table | 196,009 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 136,708 | 155,170 | 0.8810× | 0.89× | ✅ |
| task | 32,466 | 33,264 | 0.9760× | 0.96× | ✅ |
| mysql | 49,675 | 51,061 | 0.9729× | 0.97× | ✅ |
| http-client | 33,935 | 35,800 | 0.9479× | 0.94× | ✅ |
| client | 18,602 | 19,757 | 0.9415× | 0.94× | ✅ |
| socket | 19,719 | 21,073 | 0.9357× | 0.92× | ✅ |
| gzip | 194,149 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,024 | 181,572 | 0.9860× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9905× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,511 | 7,987 | 0.9405× | 0.93× | ✅ |
| connect | 17,671 | 18,707 | 0.9446× | 0.94× | ✅ |
| https | 108,152 | 108,810 | 0.9940× | 1.0× | ✅ |
| wss | 103,770 | 104,794 | 0.9902× | 0.99× | ✅ |
| atomic | 49,706 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,080 | 16,959 | 0.8892× | 0.88× | ✅ |
| http2-concurrent | 360,631 | 408,877 | 0.8820× | 0.88× | ✅ |
| websocket-64k | 89,895 | 159,694 | 0.5629× | 0.57× | ✅ |
| websocket-fragmented | 11,994 | 16,331 | 0.7344× | 0.74× | ✅ |
| upload | 412,182 | 1,322,389 | 0.3117× | 0.32× | ✅ |
| static | 27,295 | 41,045 | 0.6650× | 0.66× | ✅ |
| pgsql | 23,356 | 24,594 | 0.9497× | 0.95× | ✅ |
| sqlite | 23,548 | 25,236 | 0.9331× | 0.93× | ✅ |
| curl | 85,912 | 90,355 | 0.9508× | 0.94× | ✅ |
| dns | 230,894 | 232,761 | 0.9920× | 0.99× | ✅ |

Geometric mean 0.8574×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
