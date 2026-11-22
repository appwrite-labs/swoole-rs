# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2af5285` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,213 | 0.9044× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,185 | 0.8212× | 0.81× | ✅ |
| timer | 201,956 | 313,794 | 0.6436× | 0.65× | ✅ |
| app | 127,593 | 135,990 | 0.9383× | 0.93× | ✅ |
| http2 | 18,967 | 20,500 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,727 | 0.8821× | 0.88× | ✅ |
| task | 32,084 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,311 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,660 | 35,796 | 0.9403× | 0.93× | ✅ |
| gzip | 194,287 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,313 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,599 | 104,804 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,416 | 17,024 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,465 | 409,062 | 0.8910× | 0.89× | ✅ |
| websocket-64k | 90,010 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7282× | 0.72× | ✅ |
| upload | 416,528 | 1,322,436 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 40,999 | 0.6304× | 0.64× | ✅ |
| client | 18,621 | 19,727 | 0.9439× | 0.93× | ✅ |
| socket | 19,861 | 21,046 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,818 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,515 | 25,250 | 0.9313× | 0.93× | ✅ |
| curl | 86,130 | 90,415 | 0.9526× | 0.95× | ✅ |
| dns | 230,100 | 232,865 | 0.9881× | 0.99× | ✅ |
| file | 109,088 | 131,208 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
