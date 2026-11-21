# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bff6d1c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9250× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,341 | 17,782 | 0.9190× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,167 | 314,236 | 0.6434× | 0.65× | ✅ |
| app | 127,629 | 135,946 | 0.9388× | 0.93× | ✅ |
| http2 | 18,959 | 20,495 | 0.9251× | 0.92× | ✅ |
| table | 194,310 | 201,889 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,629 | 35,793 | 0.9395× | 0.93× | ✅ |
| gzip | 194,287 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,272 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,381 | 408,861 | 0.8912× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,528 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,877 | 41,028 | 0.6307× | 0.64× | ✅ |
| client | 18,621 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,857 | 21,036 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,824 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,134 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 230,207 | 232,808 | 0.9888× | 0.99× | ✅ |
| file | 109,096 | 131,208 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
