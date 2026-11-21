# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `db2cda7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,213 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,096 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,354 | 17,786 | 0.9195× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,945 | 313,813 | 0.6435× | 0.65× | ✅ |
| app | 127,588 | 135,944 | 0.9385× | 0.93× | ✅ |
| http2 | 18,960 | 20,496 | 0.9251× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,092 | 33,268 | 0.9647× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,674 | 35,797 | 0.9407× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,608 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,599 | 104,800 | 0.9695× | 0.97× | ✅ |
| atomic | 50,209 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,434 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,414 | 409,057 | 0.8909× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,532 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,954 | 41,092 | 0.6316× | 0.64× | ✅ |
| client | 18,630 | 19,730 | 0.9443× | 0.93× | ✅ |
| socket | 19,873 | 21,045 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,840 | 24,662 | 0.9261× | 0.92× | ✅ |
| sqlite | 23,536 | 25,272 | 0.9313× | 0.93× | ✅ |
| curl | 86,143 | 90,549 | 0.9513× | 0.95× | ✅ |
| dns | 229,983 | 232,650 | 0.9885× | 0.99× | ✅ |
| file | 109,109 | 131,296 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
