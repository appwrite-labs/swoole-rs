# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `51ca774` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,913 | 15,214 | 0.9145× | 0.92× | ✅ |
| echo-1k | 17,021 | 18,165 | 0.9370× | 0.94× | ✅ |
| json | 22,010 | 23,161 | 0.9503× | 0.95× | ✅ |
| service-health | 16,515 | 17,773 | 0.9292× | 0.93× | ✅ |
| websocket | 6,577 | 8,188 | 0.8033× | 0.82× | ✅ |
| timer | 203,545 | 314,061 | 0.6481× | 0.65× | ✅ |
| app | 125,734 | 135,928 | 0.9250× | 0.93× | ✅ |
| http2 | 18,666 | 20,464 | 0.9121× | 0.93× | ✅ |
| table | 195,468 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,395 | 154,654 | 0.8884× | 0.9× | ✅ |
| task | 32,156 | 33,264 | 0.9667× | 0.96× | ✅ |
| mysql | 49,579 | 51,064 | 0.9709× | 0.97× | ✅ |
| http-client | 33,628 | 35,809 | 0.9391× | 0.94× | ✅ |
| client | 18,472 | 19,753 | 0.9352× | 0.94× | ✅ |
| socket | 19,586 | 21,067 | 0.9297× | 0.92× | ✅ |
| gzip | 194,113 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,375 | 181,568 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,765 | 1.0047× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,231 | 0.5215× | 0.54× | ✅ |
| udp | 7,576 | 7,987 | 0.9486× | 0.95× | ✅ |
| connect | 18,352 | 18,707 | 0.9811× | 0.98× | ✅ |
| https | 108,806 | 108,806 | 1.0000× | 1.0× | ✅ |
| wss | 104,468 | 104,800 | 0.9968× | 1.0× | ✅ |
| atomic | 49,675 | 51,312 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,670 | 16,956 | 0.9832× | 0.99× | ✅ |
| http2-concurrent | 359,150 | 408,881 | 0.8784× | 0.88× | ✅ |
| websocket-64k | 156,020 | 159,697 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,327 | 0.7847× | 0.79× | ✅ |
| upload | 478,491 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,217 | 41,010 | 0.7124× | 0.72× | ✅ |
| pgsql | 23,204 | 24,594 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,487 | 25,236 | 0.9307× | 0.94× | ✅ |
| curl | 84,758 | 90,350 | 0.9381× | 0.94× | ✅ |
| dns | 230,217 | 232,889 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8819×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
