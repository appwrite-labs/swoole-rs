# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cabb2cb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,214 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9529× | 0.95× | ✅ |
| service-health | 16,344 | 17,786 | 0.9189× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,992 | 313,811 | 0.6437× | 0.65× | ✅ |
| app | 127,609 | 136,003 | 0.9383× | 0.93× | ✅ |
| http2 | 19,040 | 20,496 | 0.9289× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,365 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,332 | 51,088 | 0.9852× | 0.97× | ✅ |
| http-client | 33,644 | 35,792 | 0.9400× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,299 | 108,813 | 0.9769× | 0.98× | ✅ |
| wss | 101,579 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,207 | 51,276 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,432 | 17,021 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,613 | 408,866 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,507 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,856 | 41,005 | 0.6305× | 0.64× | ✅ |
| client | 18,640 | 19,721 | 0.9452× | 0.93× | ✅ |
| socket | 19,879 | 21,037 | 0.9450× | 0.93× | ✅ |
| pgsql | 22,821 | 24,662 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,253 | 0.9313× | 0.93× | ✅ |
| curl | 86,130 | 90,550 | 0.9512× | 0.95× | ✅ |
| dns | 230,121 | 232,496 | 0.9898× | 0.99× | ✅ |
| file | 108,940 | 130,918 | 0.8321× | 0.83× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
