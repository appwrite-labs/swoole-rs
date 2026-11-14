# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `36466f5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,953 | 15,210 | 0.9174× | 0.92× | ✅ |
| echo-1k | 17,050 | 18,165 | 0.9386× | 0.94× | ✅ |
| json | 22,116 | 23,161 | 0.9549× | 0.95× | ✅ |
| service-health | 16,557 | 17,777 | 0.9314× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 205,974 | 313,624 | 0.6568× | 0.65× | ✅ |
| app | 125,517 | 135,973 | 0.9231× | 0.93× | ✅ |
| http2 | 18,672 | 20,464 | 0.9124× | 0.91× | ✅ |
| table | 196,015 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 136,708 | 155,170 | 0.8810× | 0.89× | ✅ |
| task | 32,500 | 33,268 | 0.9769× | 0.96× | ✅ |
| mysql | 49,701 | 51,043 | 0.9737× | 0.97× | ✅ |
| http-client | 33,941 | 35,804 | 0.9480× | 0.94× | ✅ |
| client | 18,599 | 19,756 | 0.9414× | 0.94× | ✅ |
| socket | 19,723 | 21,072 | 0.9360× | 0.92× | ✅ |
| gzip | 194,162 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,100 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9905× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,508 | 7,986 | 0.9401× | 0.93× | ✅ |
| connect | 17,704 | 18,707 | 0.9464× | 0.94× | ✅ |
| https | 108,196 | 108,803 | 0.9944× | 1.0× | ✅ |
| wss | 103,854 | 104,800 | 0.9910× | 0.99× | ✅ |
| atomic | 49,706 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,100 | 17,028 | 0.8868× | 0.88× | ✅ |
| http2-concurrent | 360,801 | 409,069 | 0.8820× | 0.88× | ✅ |
| websocket-64k | 89,958 | 159,694 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 12,074 | 16,331 | 0.7393× | 0.74× | ✅ |
| upload | 412,188 | 1,322,390 | 0.3117× | 0.32× | ✅ |
| static | 27,279 | 40,993 | 0.6655× | 0.66× | ✅ |
| pgsql | 23,331 | 24,594 | 0.9486× | 0.95× | ✅ |
| sqlite | 23,531 | 25,236 | 0.9324× | 0.93× | ✅ |
| curl | 85,893 | 90,320 | 0.9510× | 0.94× | ✅ |
| dns | 231,393 | 232,780 | 0.9940× | 0.99× | ✅ |

Geometric mean 0.8580×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
