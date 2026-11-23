# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ba44b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,092 | 23,170 | 0.9535× | 0.95× | ✅ |
| service-health | 16,349 | 17,786 | 0.9192× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,937 | 313,748 | 0.6436× | 0.65× | ✅ |
| app | 127,646 | 135,955 | 0.9389× | 0.93× | ✅ |
| http2 | 18,965 | 20,493 | 0.9254× | 0.92× | ✅ |
| table | 194,318 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,370 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,265 | 0.9645× | 0.96× | ✅ |
| mysql | 50,312 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,637 | 35,787 | 0.9399× | 0.93× | ✅ |
| gzip | 194,277 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,166 | 181,565 | 0.9868× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,521 | 18,734 | 0.9352× | 0.93× | ✅ |
| https | 106,318 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,204 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,415 | 17,028 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 363,797 | 409,053 | 0.8894× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,505 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,889 | 41,013 | 0.6312× | 0.64× | ✅ |
| client | 18,629 | 19,730 | 0.9442× | 0.93× | ✅ |
| socket | 19,865 | 21,045 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,817 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,516 | 25,268 | 0.9307× | 0.93× | ✅ |
| curl | 86,122 | 90,421 | 0.9525× | 0.95× | ✅ |
| dns | 230,244 | 233,118 | 0.9877× | 0.99× | ✅ |
| file | 109,089 | 131,300 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
