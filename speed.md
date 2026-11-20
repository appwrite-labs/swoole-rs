# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,162 | 0.9258× | 0.92× | ✅ |
| json | 22,096 | 23,164 | 0.9539× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,821 | 313,791 | 0.6432× | 0.65× | ✅ |
| app | 127,655 | 135,936 | 0.9391× | 0.93× | ✅ |
| http2 | 18,956 | 20,500 | 0.9247× | 0.92× | ✅ |
| table | 194,383 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,678 | 35,794 | 0.9409× | 0.93× | ✅ |
| gzip | 194,292 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,546 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,435 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 363,588 | 409,058 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,539 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,881 | 21,045 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,832 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,138 | 90,544 | 0.9513× | 0.95× | ✅ |
| dns | 230,057 | 232,591 | 0.9891× | 0.99× | ✅ |
| file | 109,101 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
