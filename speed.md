# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `50f5654` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,748 | 15,210 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,759 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,076 | 23,164 | 0.9530× | 0.95× | ✅ |
| service-health | 16,326 | 17,777 | 0.9184× | 0.92× | ✅ |
| websocket | 6,671 | 8,192 | 0.8143× | 0.81× | ✅ |
| timer | 201,471 | 313,795 | 0.6420× | 0.65× | ✅ |
| app | 127,009 | 135,965 | 0.9341× | 0.93× | ✅ |
| http2 | 18,967 | 20,501 | 0.9252× | 0.92× | ✅ |
| table | 193,845 | 202,900 | 0.9554× | 0.96× | ✅ |
| channel | 137,345 | 155,717 | 0.8820× | 0.88× | ✅ |
| task | 31,954 | 33,268 | 0.9605× | 0.96× | ✅ |
| mysql | 50,042 | 51,085 | 0.9796× | 0.97× | ✅ |
| http-client | 33,355 | 35,795 | 0.9318× | 0.93× | ✅ |
| gzip | 194,358 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 17,588 | 18,734 | 0.9388× | 0.93× | ✅ |
| https | 106,004 | 108,813 | 0.9742× | 0.98× | ✅ |
| wss | 101,212 | 104,800 | 0.9658× | 0.97× | ✅ |
| atomic | 50,176 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,264 | 17,028 | 0.8964× | 0.89× | ✅ |
| http2-concurrent | 364,943 | 409,053 | 0.8922× | 0.89× | ✅ |
| websocket-64k | 89,969 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,368 | 0.7233× | 0.72× | ✅ |
| upload | 416,416 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,039 | 40,999 | 0.6595× | 0.65× | ✅ |
| client | 18,468 | 19,730 | 0.9361× | 0.93× | ✅ |
| socket | 19,696 | 21,041 | 0.9361× | 0.93× | ✅ |
| pgsql | 23,102 | 24,667 | 0.9366× | 0.92× | ✅ |
| sqlite | 23,436 | 25,257 | 0.9279× | 0.93× | ✅ |
| curl | 85,924 | 90,421 | 0.9503× | 0.95× | ✅ |
| dns | 229,523 | 232,800 | 0.9859× | 0.99× | ✅ |
| file | 108,217 | 131,211 | 0.8248× | 0.83× | ✅ |

Geometric mean 0.8530×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
