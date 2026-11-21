# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `257eb3e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,213 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,996 | 313,797 | 0.6437× | 0.65× | ✅ |
| app | 127,651 | 135,920 | 0.9392× | 0.93× | ✅ |
| http2 | 18,947 | 20,500 | 0.9243× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,322 | 51,085 | 0.9851× | 0.97× | ✅ |
| http-client | 33,638 | 35,796 | 0.9397× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9785× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,206 | 51,276 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,435 | 17,028 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 364,359 | 408,861 | 0.8912× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,533 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,633 | 19,721 | 0.9448× | 0.93× | ✅ |
| socket | 19,873 | 21,037 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,837 | 24,663 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,540 | 25,268 | 0.9316× | 0.93× | ✅ |
| curl | 86,144 | 90,551 | 0.9513× | 0.95× | ✅ |
| dns | 229,781 | 232,798 | 0.9870× | 0.99× | ✅ |
| file | 109,105 | 131,300 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
