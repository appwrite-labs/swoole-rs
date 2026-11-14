# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `308ac10` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,214 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,106 | 23,161 | 0.9545× | 0.95× | ✅ |
| service-health | 16,545 | 17,777 | 0.9307× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 205,963 | 313,498 | 0.6570× | 0.65× | ✅ |
| app | 125,411 | 135,863 | 0.9231× | 0.93× | ✅ |
| http2 | 18,678 | 20,468 | 0.9126× | 0.91× | ✅ |
| table | 195,564 | 201,865 | 0.9688× | 0.97× | ✅ |
| channel | 136,581 | 155,174 | 0.8802× | 0.89× | ✅ |
| task | 32,510 | 33,261 | 0.9774× | 0.96× | ✅ |
| mysql | 49,551 | 51,061 | 0.9704× | 0.97× | ✅ |
| http-client | 33,809 | 35,800 | 0.9444× | 0.94× | ✅ |
| client | 18,556 | 19,757 | 0.9392× | 0.94× | ✅ |
| socket | 19,677 | 21,072 | 0.9338× | 0.92× | ✅ |
| gzip | 194,182 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,085 | 181,568 | 0.9863× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9443× | 0.93× | ✅ |
| connect | 17,741 | 18,707 | 0.9484× | 0.94× | ✅ |
| https | 108,199 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,849 | 104,800 | 0.9909× | 0.99× | ✅ |
| atomic | 49,720 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 15,098 | 16,962 | 0.8901× | 0.88× | ✅ |
| http2-concurrent | 361,931 | 409,072 | 0.8848× | 0.88× | ✅ |
| websocket-64k | 89,957 | 159,691 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,327 | 0.7270× | 0.74× | ✅ |
| upload | 413,444 | 1,322,388 | 0.3126× | 0.32× | ✅ |
| static | 27,261 | 40,987 | 0.6651× | 0.66× | ✅ |
| pgsql | 23,374 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,121 | 90,476 | 0.9519× | 0.94× | ✅ |
| dns | 231,264 | 232,670 | 0.9940× | 0.99× | ✅ |

Geometric mean 0.8577×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
