# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `003a34c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,945 | 15,213 | 0.9167× | 0.92× | ✅ |
| echo-1k | 17,053 | 18,165 | 0.9388× | 0.94× | ✅ |
| json | 22,106 | 23,161 | 0.9544× | 0.95× | ✅ |
| service-health | 16,556 | 17,777 | 0.9313× | 0.93× | ✅ |
| websocket | 6,561 | 8,188 | 0.8013× | 0.8× | ✅ |
| timer | 207,154 | 313,707 | 0.6603× | 0.65× | ✅ |
| app | 125,591 | 135,867 | 0.9244× | 0.93× | ✅ |
| http2 | 18,657 | 20,467 | 0.9115× | 0.91× | ✅ |
| table | 196,009 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 138,030 | 155,170 | 0.8895× | 0.89× | ✅ |
| task | 32,217 | 33,265 | 0.9685× | 0.96× | ✅ |
| mysql | 49,695 | 51,055 | 0.9734× | 0.97× | ✅ |
| http-client | 33,914 | 35,804 | 0.9472× | 0.94× | ✅ |
| client | 18,598 | 19,757 | 0.9414× | 0.94× | ✅ |
| socket | 19,728 | 21,072 | 0.9362× | 0.92× | ✅ |
| gzip | 194,181 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,050 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,714 | 4,762 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,207 | 10,231 | 0.5089× | 0.51× | ✅ |
| udp | 7,388 | 7,986 | 0.9251× | 0.93× | ✅ |
| connect | 17,694 | 18,703 | 0.9460× | 0.94× | ✅ |
| https | 108,220 | 108,810 | 0.9946× | 1.0× | ✅ |
| wss | 103,808 | 104,794 | 0.9906× | 0.99× | ✅ |
| atomic | 49,712 | 51,316 | 0.9687× | 0.97× | ✅ |
| process-mode | 15,082 | 16,959 | 0.8893× | 0.88× | ✅ |
| http2-concurrent | 359,246 | 409,073 | 0.8782× | 0.88× | ✅ |
| websocket-64k | 89,903 | 159,699 | 0.5629× | 0.57× | ✅ |
| websocket-fragmented | 12,025 | 16,327 | 0.7365× | 0.74× | ✅ |
| upload | 412,409 | 1,322,387 | 0.3119× | 0.32× | ✅ |
| static | 27,268 | 41,080 | 0.6638× | 0.66× | ✅ |
| pgsql | 23,347 | 24,594 | 0.9493× | 0.95× | ✅ |
| sqlite | 23,560 | 25,236 | 0.9336× | 0.93× | ✅ |
| curl | 85,757 | 90,350 | 0.9492× | 0.94× | ✅ |
| dns | 231,360 | 233,108 | 0.9925× | 0.99× | ✅ |

Geometric mean 0.8572×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
