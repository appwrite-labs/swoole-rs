# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `83c55af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,162 | 0.9253× | 0.92× | ✅ |
| json | 22,097 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,354 | 17,782 | 0.9197× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,931 | 313,944 | 0.6432× | 0.65× | ✅ |
| app | 127,624 | 135,946 | 0.9388× | 0.93× | ✅ |
| http2 | 18,957 | 20,500 | 0.9247× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,636 | 35,793 | 0.9397× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,599 | 104,806 | 0.9694× | 0.97× | ✅ |
| atomic | 50,209 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,320 | 409,057 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,528 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,896 | 41,005 | 0.6315× | 0.64× | ✅ |
| client | 18,634 | 19,730 | 0.9445× | 0.93× | ✅ |
| socket | 19,873 | 21,046 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,830 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,532 | 25,268 | 0.9313× | 0.93× | ✅ |
| curl | 86,142 | 90,422 | 0.9527× | 0.95× | ✅ |
| dns | 230,190 | 232,831 | 0.9887× | 0.99× | ✅ |
| file | 109,097 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
