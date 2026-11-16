# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f963095` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,793 | 15,210 | 0.9069× | 0.92× | ✅ |
| echo-1k | 16,824 | 18,162 | 0.9263× | 0.94× | ✅ |
| json | 21,938 | 23,161 | 0.9472× | 0.95× | ✅ |
| service-health | 16,380 | 17,773 | 0.9216× | 0.93× | ✅ |
| websocket | 6,547 | 8,191 | 0.7993× | 0.8× | ✅ |
| timer | 206,082 | 313,532 | 0.6573× | 0.65× | ✅ |
| app | 125,324 | 135,942 | 0.9219× | 0.93× | ✅ |
| http2 | 18,813 | 20,465 | 0.9193× | 0.91× | ✅ |
| table | 194,978 | 201,865 | 0.9659× | 0.97× | ✅ |
| channel | 136,685 | 155,170 | 0.8809× | 0.88× | ✅ |
| task | 32,302 | 33,268 | 0.9710× | 0.96× | ✅ |
| mysql | 49,414 | 51,064 | 0.9677× | 0.97× | ✅ |
| http-client | 33,351 | 35,804 | 0.9315× | 0.93× | ✅ |
| gzip | 193,948 | 197,079 | 0.9841× | 0.99× | ✅ |
| websocket-deflate | 178,798 | 181,568 | 0.9847× | 0.99× | ✅ |
| tcp-length | 4,650 | 4,762 | 0.9765× | 0.98× | ✅ |
| tcp-eof | 5,145 | 10,231 | 0.5029× | 0.51× | ✅ |
| udp | 7,483 | 7,986 | 0.9369× | 0.93× | ✅ |
| connect | 17,566 | 18,703 | 0.9392× | 0.94× | ✅ |
| https | 108,035 | 108,813 | 0.9928× | 1.0× | ✅ |
| wss | 103,728 | 104,797 | 0.9898× | 0.99× | ✅ |
| atomic | 49,641 | 51,316 | 0.9674× | 0.97× | ✅ |
| process-mode | 14,948 | 16,959 | 0.8814× | 0.88× | ✅ |
| http2-concurrent | 365,231 | 408,881 | 0.8932× | 0.88× | ✅ |
| websocket-64k | 89,837 | 159,695 | 0.5626× | 0.57× | ✅ |
| websocket-fragmented | 11,740 | 16,327 | 0.7190× | 0.73× | ✅ |
| upload | 414,042 | 1,322,388 | 0.3131× | 0.32× | ✅ |
| static | 27,155 | 41,045 | 0.6616× | 0.66× | ✅ |
| client | 18,423 | 19,756 | 0.9325× | 0.94× | ✅ |
| socket | 19,568 | 21,073 | 0.9286× | 0.92× | ✅ |
| pgsql | 23,222 | 24,594 | 0.9442× | 0.95× | ✅ |
| sqlite | 23,441 | 25,221 | 0.9294× | 0.93× | ✅ |
| curl | 85,569 | 90,350 | 0.9471× | 0.94× | ✅ |
| dns | 230,157 | 232,670 | 0.9892× | 0.99× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
