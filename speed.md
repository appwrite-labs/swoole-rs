# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `970915f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,750 | 15,214 | 0.9038× | 0.91× | ✅ |
| echo-1k | 16,738 | 18,165 | 0.9214× | 0.92× | ✅ |
| json | 22,070 | 23,173 | 0.9524× | 0.95× | ✅ |
| service-health | 16,314 | 17,786 | 0.9172× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8153× | 0.81× | ✅ |
| timer | 201,542 | 313,898 | 0.6421× | 0.65× | ✅ |
| app | 127,039 | 135,894 | 0.9348× | 0.93× | ✅ |
| http2 | 18,972 | 20,497 | 0.9256× | 0.92× | ✅ |
| table | 193,906 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 137,976 | 155,723 | 0.8860× | 0.88× | ✅ |
| task | 31,949 | 33,268 | 0.9603× | 0.96× | ✅ |
| mysql | 50,064 | 51,085 | 0.9800× | 0.97× | ✅ |
| http-client | 33,405 | 35,796 | 0.9332× | 0.93× | ✅ |
| gzip | 194,360 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,190 | 10,231 | 0.5073× | 0.5× | ✅ |
| udp | 7,579 | 7,987 | 0.9489× | 0.95× | ✅ |
| connect | 17,598 | 18,734 | 0.9393× | 0.93× | ✅ |
| https | 106,034 | 108,802 | 0.9746× | 0.98× | ✅ |
| wss | 101,244 | 104,803 | 0.9660× | 0.97× | ✅ |
| atomic | 50,176 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,279 | 17,024 | 0.8975× | 0.89× | ✅ |
| http2-concurrent | 364,531 | 409,057 | 0.8912× | 0.89× | ✅ |
| websocket-64k | 89,978 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,248 | 1,322,429 | 0.3148× | 0.32× | ✅ |
| static | 27,047 | 40,999 | 0.6597× | 0.65× | ✅ |
| client | 18,475 | 19,730 | 0.9364× | 0.93× | ✅ |
| socket | 19,699 | 21,046 | 0.9360× | 0.93× | ✅ |
| pgsql | 22,907 | 24,618 | 0.9305× | 0.92× | ✅ |
| sqlite | 23,417 | 25,236 | 0.9279× | 0.93× | ✅ |
| curl | 85,356 | 90,357 | 0.9447× | 0.95× | ✅ |
| dns | 229,976 | 233,012 | 0.9870× | 0.99× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
