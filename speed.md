# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2adebbf` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,213 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,165 | 0.9252× | 0.92× | ✅ |
| json | 22,096 | 23,164 | 0.9539× | 0.95× | ✅ |
| service-health | 16,354 | 17,777 | 0.9200× | 0.92× | ✅ |
| websocket | 6,741 | 8,185 | 0.8236× | 0.81× | ✅ |
| timer | 202,028 | 313,845 | 0.6437× | 0.65× | ✅ |
| app | 127,610 | 135,996 | 0.9383× | 0.93× | ✅ |
| http2 | 18,969 | 20,497 | 0.9255× | 0.92× | ✅ |
| table | 193,875 | 202,900 | 0.9555× | 0.96× | ✅ |
| channel | 137,375 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,315 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,628 | 35,800 | 0.9393× | 0.93× | ✅ |
| gzip | 194,415 | 197,095 | 0.9864× | 0.99× | ✅ |
| websocket-deflate | 179,149 | 181,573 | 0.9866× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,089 | 10,231 | 0.4974× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,737 | 18,734 | 0.9468× | 0.93× | ✅ |
| https | 106,324 | 108,813 | 0.9771× | 0.98× | ✅ |
| wss | 101,603 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,209 | 51,281 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,389 | 17,024 | 0.9040× | 0.89× | ✅ |
| http2-concurrent | 363,357 | 409,057 | 0.8883× | 0.89× | ✅ |
| websocket-64k | 90,017 | 159,694 | 0.5637× | 0.57× | ✅ |
| websocket-fragmented | 11,953 | 16,368 | 0.7303× | 0.72× | ✅ |
| upload | 416,406 | 1,322,432 | 0.3149× | 0.32× | ✅ |
| static | 26,367 | 40,999 | 0.6431× | 0.65× | ✅ |
| client | 18,627 | 19,730 | 0.9441× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 23,051 | 24,663 | 0.9346× | 0.92× | ✅ |
| sqlite | 23,488 | 25,268 | 0.9296× | 0.93× | ✅ |
| curl | 85,920 | 90,417 | 0.9503× | 0.95× | ✅ |
| dns | 229,784 | 232,693 | 0.9875× | 0.99× | ✅ |
| file | 109,111 | 131,207 | 0.8316× | 0.83× | ✅ |

Geometric mean 0.8545×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
