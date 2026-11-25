# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4e89023` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,757 | 15,214 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,090 | 23,173 | 0.9532× | 0.95× | ✅ |
| service-health | 16,348 | 17,786 | 0.9191× | 0.92× | ✅ |
| websocket | 6,723 | 8,192 | 0.8207× | 0.81× | ✅ |
| timer | 202,151 | 314,096 | 0.6436× | 0.65× | ✅ |
| app | 127,614 | 136,004 | 0.9383× | 0.93× | ✅ |
| http2 | 19,087 | 20,497 | 0.9312× | 0.92× | ✅ |
| table | 194,320 | 201,889 | 0.9625× | 0.96× | ✅ |
| channel | 137,376 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,082 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,331 | 51,085 | 0.9852× | 0.97× | ✅ |
| http-client | 33,662 | 35,788 | 0.9406× | 0.93× | ✅ |
| gzip | 194,284 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,212 | 181,565 | 0.9870× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,308 | 108,806 | 0.9770× | 0.98× | ✅ |
| wss | 101,583 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,203 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,432 | 17,028 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 366,006 | 409,058 | 0.8948× | 0.89× | ✅ |
| websocket-64k | 90,007 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,372 | 0.7284× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 41,028 | 0.6300× | 0.64× | ✅ |
| client | 18,639 | 19,730 | 0.9447× | 0.93× | ✅ |
| socket | 19,878 | 21,046 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,821 | 24,664 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,128 | 90,550 | 0.9512× | 0.95× | ✅ |
| dns | 229,488 | 232,600 | 0.9866× | 0.99× | ✅ |
| file | 108,940 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
