# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `777bbff` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,207 | 0.9185× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,162 | 0.9397× | 0.94× | ✅ |
| json | 22,128 | 23,161 | 0.9554× | 0.95× | ✅ |
| service-health | 16,567 | 17,773 | 0.9321× | 0.93× | ✅ |
| websocket | 6,636 | 8,191 | 0.8102× | 0.8× | ✅ |
| timer | 205,931 | 313,503 | 0.6569× | 0.65× | ✅ |
| app | 125,380 | 135,951 | 0.9222× | 0.93× | ✅ |
| http2 | 18,702 | 20,465 | 0.9139× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,167 | 0.8770× | 0.89× | ✅ |
| task | 32,506 | 33,268 | 0.9771× | 0.96× | ✅ |
| mysql | 49,553 | 51,064 | 0.9704× | 0.97× | ✅ |
| http-client | 33,814 | 35,804 | 0.9444× | 0.94× | ✅ |
| client | 18,553 | 19,757 | 0.9391× | 0.94× | ✅ |
| socket | 19,677 | 21,072 | 0.9338× | 0.92× | ✅ |
| gzip | 194,170 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,568 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,228 | 0.5094× | 0.51× | ✅ |
| udp | 7,543 | 7,987 | 0.9444× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,212 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,849 | 104,794 | 0.9910× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,098 | 17,021 | 0.8870× | 0.88× | ✅ |
| http2-concurrent | 361,578 | 409,069 | 0.8839× | 0.88× | ✅ |
| websocket-64k | 89,961 | 159,692 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,331 | 0.7269× | 0.73× | ✅ |
| upload | 413,458 | 1,322,388 | 0.3127× | 0.32× | ✅ |
| static | 27,313 | 41,051 | 0.6653× | 0.66× | ✅ |
| pgsql | 23,373 | 24,595 | 0.9503× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,125 | 90,420 | 0.9525× | 0.94× | ✅ |
| dns | 231,197 | 233,010 | 0.9922× | 0.99× | ✅ |

Geometric mean 0.8577×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
