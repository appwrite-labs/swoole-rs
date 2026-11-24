# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bc25fd4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,210 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,804 | 18,165 | 0.9250× | 0.92× | ✅ |
| json | 22,086 | 23,173 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9187× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,981 | 313,790 | 0.6437× | 0.65× | ✅ |
| app | 127,667 | 135,988 | 0.9388× | 0.93× | ✅ |
| http2 | 18,973 | 20,500 | 0.9255× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,365 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,084 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,283 | 51,085 | 0.9843× | 0.97× | ✅ |
| http-client | 33,676 | 35,788 | 0.9410× | 0.93× | ✅ |
| gzip | 194,278 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,764 | 0.9784× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,543 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,321 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,596 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,199 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,421 | 17,024 | 0.9058× | 0.89× | ✅ |
| http2-concurrent | 363,606 | 409,057 | 0.8889× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,523 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,916 | 41,092 | 0.6307× | 0.64× | ✅ |
| client | 18,627 | 19,730 | 0.9441× | 0.93× | ✅ |
| socket | 19,863 | 21,046 | 0.9438× | 0.93× | ✅ |
| pgsql | 22,830 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,141 | 90,422 | 0.9527× | 0.95× | ✅ |
| dns | 229,638 | 232,748 | 0.9866× | 0.99× | ✅ |
| file | 109,182 | 131,300 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
