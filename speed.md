# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b0c7d6d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,804 | 15,210 | 0.9076× | 0.92× | ✅ |
| echo-1k | 16,844 | 18,162 | 0.9275× | 0.94× | ✅ |
| json | 21,956 | 23,158 | 0.9481× | 0.95× | ✅ |
| service-health | 16,391 | 17,773 | 0.9222× | 0.93× | ✅ |
| websocket | 6,535 | 8,191 | 0.7978× | 0.8× | ✅ |
| timer | 206,329 | 313,583 | 0.6580× | 0.65× | ✅ |
| app | 125,403 | 135,907 | 0.9227× | 0.93× | ✅ |
| http2 | 18,817 | 20,464 | 0.9195× | 0.91× | ✅ |
| table | 194,989 | 201,865 | 0.9659× | 0.97× | ✅ |
| channel | 136,756 | 155,170 | 0.8813× | 0.88× | ✅ |
| task | 32,302 | 33,268 | 0.9710× | 0.96× | ✅ |
| mysql | 49,418 | 51,062 | 0.9678× | 0.97× | ✅ |
| http-client | 33,412 | 35,800 | 0.9333× | 0.93× | ✅ |
| gzip | 193,923 | 197,079 | 0.9840× | 0.99× | ✅ |
| websocket-deflate | 178,757 | 181,568 | 0.9845× | 0.99× | ✅ |
| tcp-length | 4,647 | 4,765 | 0.9752× | 0.98× | ✅ |
| tcp-eof | 5,142 | 10,231 | 0.5026× | 0.51× | ✅ |
| udp | 7,485 | 7,986 | 0.9373× | 0.93× | ✅ |
| connect | 17,567 | 18,703 | 0.9393× | 0.94× | ✅ |
| https | 108,062 | 108,810 | 0.9931× | 1.0× | ✅ |
| wss | 103,721 | 104,797 | 0.9897× | 0.99× | ✅ |
| atomic | 49,908 | 51,316 | 0.9726× | 0.97× | ✅ |
| process-mode | 14,984 | 17,024 | 0.8801× | 0.88× | ✅ |
| http2-concurrent | 357,031 | 409,074 | 0.8728× | 0.88× | ✅ |
| websocket-64k | 89,822 | 159,692 | 0.5625× | 0.57× | ✅ |
| websocket-fragmented | 11,683 | 16,331 | 0.7154× | 0.73× | ✅ |
| upload | 414,061 | 1,322,387 | 0.3131× | 0.32× | ✅ |
| static | 26,944 | 41,080 | 0.6559× | 0.66× | ✅ |
| client | 18,436 | 19,753 | 0.9333× | 0.94× | ✅ |
| socket | 19,594 | 21,072 | 0.9298× | 0.92× | ✅ |
| pgsql | 23,240 | 24,595 | 0.9449× | 0.95× | ✅ |
| sqlite | 23,456 | 25,236 | 0.9295× | 0.93× | ✅ |
| curl | 85,587 | 90,351 | 0.9473× | 0.94× | ✅ |
| dns | 230,412 | 232,766 | 0.9899× | 0.99× | ✅ |

Geometric mean 0.8527×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
