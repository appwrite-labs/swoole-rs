# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `83c55af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,165 | 0.9251× | 0.92× | ✅ |
| json | 22,096 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,020 | 313,718 | 0.6440× | 0.65× | ✅ |
| app | 127,607 | 135,953 | 0.9386× | 0.93× | ✅ |
| http2 | 18,947 | 20,500 | 0.9243× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,096 | 33,265 | 0.9649× | 0.96× | ✅ |
| mysql | 50,313 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,681 | 35,788 | 0.9411× | 0.93× | ✅ |
| gzip | 194,280 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,323 | 108,802 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,793 | 0.9695× | 0.97× | ✅ |
| atomic | 50,209 | 51,276 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 364,548 | 408,865 | 0.8916× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,533 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,867 | 41,005 | 0.6308× | 0.64× | ✅ |
| client | 18,630 | 19,721 | 0.9447× | 0.93× | ✅ |
| socket | 19,873 | 21,036 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,838 | 24,663 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,540 | 25,268 | 0.9316× | 0.93× | ✅ |
| curl | 86,144 | 90,549 | 0.9514× | 0.95× | ✅ |
| dns | 230,048 | 232,816 | 0.9881× | 0.99× | ✅ |
| file | 109,109 | 130,922 | 0.8334× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
