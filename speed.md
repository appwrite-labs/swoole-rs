# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,214 | 0.9049× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,165 | 0.9248× | 0.92× | ✅ |
| json | 22,094 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,349 | 17,786 | 0.9192× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,957 | 313,785 | 0.6436× | 0.65× | ✅ |
| app | 127,525 | 135,983 | 0.9378× | 0.93× | ✅ |
| http2 | 19,009 | 20,496 | 0.9274× | 0.92× | ✅ |
| table | 194,321 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,373 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,091 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,293 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,604 | 35,793 | 0.9388× | 0.93× | ✅ |
| gzip | 194,297 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,300 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,207 | 51,281 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,436 | 17,021 | 0.9069× | 0.89× | ✅ |
| http2-concurrent | 363,766 | 409,053 | 0.8893× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 416,518 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,862 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,614 | 19,729 | 0.9435× | 0.93× | ✅ |
| socket | 19,857 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,134 | 90,422 | 0.9526× | 0.95× | ✅ |
| dns | 230,033 | 232,829 | 0.9880× | 0.99× | ✅ |
| file | 108,937 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
