# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1478de6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,772 | 15,214 | 0.9710× | 0.95× | ✅ |
| echo-1k | 17,954 | 18,166 | 0.9884× | 0.96× | ✅ |
| json | 22,803 | 23,178 | 0.9838× | 0.97× | ✅ |
| service-health | 17,363 | 17,795 | 0.9757× | 0.96× | ✅ |
| websocket | 6,948 | 8,188 | 0.8486× | 0.81× | ✅ |
| timer | 201,672 | 313,373 | 0.6436× | 0.65× | ✅ |
| app | 127,671 | 135,934 | 0.9392× | 0.92× | ✅ |
| http2 | 20,448 | 20,468 | 0.9990× | 0.97× | ✅ |
| table | 197,734 | 202,907 | 0.9745× | 0.98× | ✅ |
| channel | 140,376 | 155,192 | 0.9045× | 0.89× | ✅ |
| task | 32,849 | 33,273 | 0.9873× | 0.96× | ✅ |
| mysql | 50,418 | 51,079 | 0.9871× | 0.97× | ✅ |
| http-client | 34,894 | 35,824 | 0.9740× | 0.94× | ✅ |
| client | 19,303 | 19,775 | 0.9761× | 0.95× | ✅ |
| socket | 20,805 | 21,090 | 0.9865× | 0.99× | ✅ |
| gzip | 197,124 | 197,093 | 1.0002× | 1.02× | ➖ |
| websocket-deflate | 181,743 | 181,560 | 1.0010× | 1.0× | ❌ |
| tcp-length | 5,476 | 4,743 | 1.1544× | 1.07× | ❌ |
| tcp-eof | 5,898 | 10,212 | 0.5775× | 0.55× | ✅ |
| udp | 7,181 | 7,337 | 0.9787× | 1.04× | ✅ |
| connect | 21,673 | 18,703 | 1.1588× | 1.09× | ❌ |
| https | 110,166 | 108,814 | 1.0124× | 1.01× | ❌ |
| wss | 105,369 | 104,797 | 1.0055× | 1.0× | ❌ |
| atomic | 53,217 | 51,334 | 1.0367× | 1.06× | ➖ |
| process-mode | 17,181 | 16,945 | 1.0139× | 0.98× | ❌ |
| http2-concurrent | 444,253 | 408,817 | 1.0867× | 0.93× | ❌ |
| websocket-64k | 156,400 | 159,691 | 0.9794× | 0.62× | ✅ |
| websocket-fragmented | 14,182 | 16,194 | 0.8758× | 0.78× | ✅ |
| upload | 674,831 | 1,322,425 | 0.5103× | 1.05× | ✅ |
| static | 40,896 | 41,028 | 0.9968× | 0.96× | ✅ |

Geometric mean 0.9379×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
