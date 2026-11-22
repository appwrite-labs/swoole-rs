# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be7677e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,221 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,167 | 0.9532× | 0.95× | ✅ |
| service-health | 16,341 | 17,780 | 0.9191× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,028 | 313,749 | 0.6439× | 0.65× | ✅ |
| app | 127,693 | 136,058 | 0.9385× | 0.93× | ✅ |
| http2 | 18,966 | 20,504 | 0.9250× | 0.92× | ✅ |
| table | 193,862 | 201,904 | 0.9602× | 0.96× | ✅ |
| channel | 137,381 | 154,732 | 0.8879× | 0.88× | ✅ |
| task | 32,103 | 33,312 | 0.9637× | 0.96× | ✅ |
| mysql | 50,313 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,624 | 35,818 | 0.9388× | 0.93× | ✅ |
| gzip | 194,291 | 197,103 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,742 | 0.9355× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,597 | 104,812 | 0.9693× | 0.97× | ✅ |
| atomic | 50,196 | 51,304 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,413 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,301 | 409,000 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,699 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,921 | 16,376 | 0.7280× | 0.72× | ✅ |
| upload | 416,526 | 1,322,435 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,069 | 0.6309× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,034 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,820 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,527 | 25,264 | 0.9312× | 0.93× | ✅ |
| curl | 86,127 | 90,422 | 0.9525× | 0.95× | ✅ |
| dns | 230,096 | 233,225 | 0.9866× | 0.99× | ✅ |
| file | 109,087 | 131,295 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
