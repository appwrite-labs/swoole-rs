# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,970 | 15,216 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,055 | 18,161 | 0.9391× | 0.94× | ✅ |
| json | 22,121 | 23,158 | 0.9552× | 0.95× | ✅ |
| service-health | 16,570 | 17,771 | 0.9325× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,905 | 313,568 | 0.6567× | 0.65× | ✅ |
| app | 125,467 | 135,993 | 0.9226× | 0.93× | ✅ |
| http2 | 18,674 | 20,490 | 0.9114× | 0.91× | ✅ |
| table | 195,578 | 201,916 | 0.9686× | 0.97× | ✅ |
| channel | 136,083 | 154,724 | 0.8795× | 0.88× | ✅ |
| task | 32,526 | 33,311 | 0.9764× | 0.96× | ✅ |
| mysql | 49,577 | 51,046 | 0.9712× | 0.97× | ✅ |
| http-client | 33,824 | 35,822 | 0.9442× | 0.94× | ✅ |
| client | 18,557 | 19,757 | 0.9393× | 0.94× | ✅ |
| socket | 19,673 | 21,062 | 0.9341× | 0.92× | ✅ |
| gzip | 194,175 | 197,084 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,546 | 7,987 | 0.9449× | 0.93× | ✅ |
| connect | 17,748 | 18,714 | 0.9484× | 0.94× | ✅ |
| https | 108,199 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,855 | 104,794 | 0.9910× | 0.99× | ✅ |
| atomic | 49,702 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,094 | 17,028 | 0.8864× | 0.88× | ✅ |
| http2-concurrent | 361,916 | 409,146 | 0.8846× | 0.88× | ✅ |
| websocket-64k | 89,957 | 159,694 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,869 | 16,328 | 0.7269× | 0.73× | ✅ |
| upload | 413,032 | 1,322,393 | 0.3123× | 0.32× | ✅ |
| static | 27,330 | 41,074 | 0.6654× | 0.66× | ✅ |
| pgsql | 23,375 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,537 | 25,232 | 0.9328× | 0.93× | ✅ |
| curl | 86,125 | 90,325 | 0.9535× | 0.94× | ✅ |
| dns | 231,344 | 232,726 | 0.9941× | 0.99× | ✅ |

Geometric mean 0.8577×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
