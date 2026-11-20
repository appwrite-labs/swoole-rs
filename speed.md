# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9f21772` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,166 | 0.9251× | 0.92× | ✅ |
| json | 22,093 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,354 | 17,786 | 0.9195× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,981 | 313,720 | 0.6438× | 0.65× | ✅ |
| app | 127,675 | 135,984 | 0.9389× | 0.93× | ✅ |
| http2 | 18,970 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,085 | 33,267 | 0.9645× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,638 | 35,792 | 0.9398× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,534 | 18,734 | 0.9360× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,428 | 17,024 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 363,670 | 409,053 | 0.8891× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,530 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,861 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,634 | 19,730 | 0.9444× | 0.93× | ✅ |
| socket | 19,873 | 21,041 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,832 | 24,662 | 0.9258× | 0.92× | ✅ |
| sqlite | 23,532 | 25,268 | 0.9313× | 0.93× | ✅ |
| curl | 86,136 | 90,422 | 0.9526× | 0.95× | ✅ |
| dns | 230,367 | 232,730 | 0.9898× | 0.99× | ✅ |
| file | 109,101 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
