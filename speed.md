# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9928313` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 202,003 | 313,712 | 0.6439× | 0.65× | ✅ |
| app | 127,610 | 135,976 | 0.9385× | 0.93× | ✅ |
| http2 | 18,958 | 20,500 | 0.9248× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,674 | 35,797 | 0.9407× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,435 | 17,028 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 363,651 | 409,056 | 0.8890× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,532 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,890 | 41,028 | 0.6310× | 0.64× | ✅ |
| client | 18,634 | 19,730 | 0.9445× | 0.93× | ✅ |
| socket | 19,873 | 21,045 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,829 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,528 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,145 | 90,417 | 0.9527× | 0.95× | ✅ |
| dns | 230,164 | 233,049 | 0.9876× | 0.99× | ✅ |
| file | 109,101 | 131,296 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
