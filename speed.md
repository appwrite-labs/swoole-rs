# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `585efdd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,953 | 15,214 | 0.9171× | 0.92× | ✅ |
| echo-1k | 17,069 | 18,165 | 0.9397× | 0.94× | ✅ |
| json | 22,155 | 23,158 | 0.9567× | 0.95× | ✅ |
| service-health | 16,592 | 17,777 | 0.9333× | 0.93× | ✅ |
| websocket | 6,568 | 8,188 | 0.8021× | 0.8× | ✅ |
| timer | 207,279 | 313,751 | 0.6606× | 0.65× | ✅ |
| app | 125,659 | 135,902 | 0.9246× | 0.93× | ✅ |
| http2 | 18,679 | 20,464 | 0.9128× | 0.91× | ✅ |
| table | 196,044 | 201,865 | 0.9712× | 0.97× | ✅ |
| channel | 137,547 | 155,170 | 0.8864× | 0.89× | ✅ |
| task | 32,493 | 33,265 | 0.9768× | 0.96× | ✅ |
| mysql | 49,566 | 51,061 | 0.9707× | 0.97× | ✅ |
| http-client | 33,974 | 35,801 | 0.9490× | 0.94× | ✅ |
| client | 18,631 | 19,753 | 0.9432× | 0.94× | ✅ |
| socket | 19,754 | 21,072 | 0.9375× | 0.92× | ✅ |
| gzip | 194,181 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,051 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,729 | 4,765 | 0.9924× | 0.99× | ✅ |
| tcp-eof | 5,222 | 10,231 | 0.5104× | 0.51× | ✅ |
| udp | 7,503 | 7,987 | 0.9395× | 0.93× | ✅ |
| connect | 17,711 | 18,707 | 0.9468× | 0.94× | ✅ |
| https | 108,214 | 108,813 | 0.9945× | 1.0× | ✅ |
| wss | 103,802 | 104,794 | 0.9905× | 0.99× | ✅ |
| atomic | 49,744 | 51,316 | 0.9694× | 0.97× | ✅ |
| process-mode | 15,080 | 16,956 | 0.8894× | 0.88× | ✅ |
| http2-concurrent | 360,058 | 409,621 | 0.8790× | 0.88× | ✅ |
| websocket-64k | 89,908 | 159,701 | 0.5630× | 0.57× | ✅ |
| websocket-fragmented | 12,025 | 16,327 | 0.7365× | 0.74× | ✅ |
| upload | 412,216 | 1,322,387 | 0.3117× | 0.32× | ✅ |
| static | 27,174 | 41,051 | 0.6620× | 0.66× | ✅ |
| pgsql | 23,393 | 24,594 | 0.9512× | 0.95× | ✅ |
| sqlite | 23,592 | 25,236 | 0.9348× | 0.93× | ✅ |
| curl | 86,143 | 90,354 | 0.9534× | 0.94× | ✅ |
| dns | 231,141 | 232,841 | 0.9927× | 0.99× | ✅ |

Geometric mean 0.8583×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
