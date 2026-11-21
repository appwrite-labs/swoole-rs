# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2acfc42` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,164 | 0.9538× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,978 | 313,834 | 0.6436× | 0.65× | ✅ |
| app | 127,653 | 135,900 | 0.9393× | 0.93× | ✅ |
| http2 | 18,968 | 20,500 | 0.9253× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,092 | 33,265 | 0.9647× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,673 | 35,797 | 0.9407× | 0.93× | ✅ |
| gzip | 194,292 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,608 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,206 | 51,276 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 364,500 | 408,861 | 0.8915× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,529 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,636 | 19,718 | 0.9452× | 0.93× | ✅ |
| socket | 19,873 | 21,036 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,838 | 24,663 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,540 | 25,268 | 0.9316× | 0.93× | ✅ |
| curl | 86,145 | 90,419 | 0.9527× | 0.95× | ✅ |
| dns | 229,761 | 232,748 | 0.9872× | 0.99× | ✅ |
| file | 109,104 | 131,207 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
