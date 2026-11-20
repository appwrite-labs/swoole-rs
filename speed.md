# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cc86038` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,813 | 18,162 | 0.9257× | 0.92× | ✅ |
| json | 22,096 | 23,170 | 0.9537× | 0.95× | ✅ |
| service-health | 16,354 | 17,782 | 0.9197× | 0.92× | ✅ |
| websocket | 6,725 | 8,192 | 0.8209× | 0.81× | ✅ |
| timer | 202,038 | 313,745 | 0.6440× | 0.65× | ✅ |
| app | 127,641 | 135,941 | 0.9389× | 0.93× | ✅ |
| http2 | 18,963 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,379 | 155,727 | 0.8822× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,671 | 35,795 | 0.9407× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,806 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,209 | 51,281 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,426 | 17,024 | 0.9061× | 0.89× | ✅ |
| http2-concurrent | 364,359 | 409,053 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,514 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,873 | 40,999 | 0.6311× | 0.64× | ✅ |
| client | 18,634 | 19,730 | 0.9445× | 0.93× | ✅ |
| socket | 19,873 | 21,045 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,839 | 24,663 | 0.9261× | 0.92× | ✅ |
| sqlite | 23,536 | 25,268 | 0.9315× | 0.93× | ✅ |
| curl | 86,141 | 90,547 | 0.9513× | 0.95× | ✅ |
| dns | 230,346 | 232,843 | 0.9893× | 0.99× | ✅ |
| file | 109,109 | 131,211 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
