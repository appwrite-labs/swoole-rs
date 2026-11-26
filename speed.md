# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8281d93` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,746 | 15,210 | 0.9037× | 0.91× | ✅ |
| echo-1k | 16,785 | 18,165 | 0.9240× | 0.92× | ✅ |
| json | 22,070 | 23,173 | 0.9524× | 0.95× | ✅ |
| service-health | 16,325 | 17,786 | 0.9178× | 0.92× | ✅ |
| websocket | 6,723 | 8,185 | 0.8214× | 0.81× | ✅ |
| timer | 202,160 | 313,842 | 0.6441× | 0.65× | ✅ |
| app | 127,572 | 135,993 | 0.9381× | 0.93× | ✅ |
| http2 | 19,160 | 20,497 | 0.9348× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,043 | 155,726 | 0.8800× | 0.88× | ✅ |
| task | 32,109 | 33,265 | 0.9652× | 0.96× | ✅ |
| mysql | 50,319 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,678 | 35,794 | 0.9409× | 0.93× | ✅ |
| gzip | 194,272 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,987 | 181,573 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,513 | 18,734 | 0.9348× | 0.93× | ✅ |
| https | 106,267 | 108,802 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,797 | 0.9691× | 0.97× | ✅ |
| atomic | 50,440 | 51,285 | 0.9835× | 0.97× | ✅ |
| process-mode | 15,430 | 17,024 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 367,126 | 409,057 | 0.8975× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,372 | 0.7286× | 0.72× | ✅ |
| upload | 418,870 | 1,322,429 | 0.3167× | 0.32× | ✅ |
| static | 25,319 | 41,005 | 0.6174× | 0.62× | ✅ |
| client | 18,617 | 19,730 | 0.9436× | 0.93× | ✅ |
| socket | 19,852 | 21,046 | 0.9433× | 0.93× | ✅ |
| pgsql | 22,917 | 24,662 | 0.9293× | 0.92× | ✅ |
| sqlite | 23,506 | 25,253 | 0.9308× | 0.93× | ✅ |
| curl | 86,103 | 90,549 | 0.9509× | 0.95× | ✅ |
| dns | 230,354 | 232,611 | 0.9903× | 0.99× | ✅ |
| file | 108,913 | 131,300 | 0.8295× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
