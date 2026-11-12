# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4e9390c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,931 | 15,213 | 0.9157× | 0.92× | ✅ |
| echo-1k | 17,050 | 18,165 | 0.9386× | 0.94× | ✅ |
| json | 22,026 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,526 | 17,777 | 0.9296× | 0.93× | ✅ |
| websocket | 6,732 | 8,188 | 0.8223× | 0.82× | ✅ |
| timer | 202,825 | 313,843 | 0.6463× | 0.65× | ✅ |
| app | 125,983 | 135,937 | 0.9268× | 0.93× | ✅ |
| http2 | 19,262 | 20,468 | 0.9411× | 0.94× | ✅ |
| table | 195,734 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,876 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 31,953 | 33,268 | 0.9605× | 0.96× | ✅ |
| mysql | 49,576 | 51,064 | 0.9709× | 0.97× | ✅ |
| http-client | 33,623 | 35,803 | 0.9391× | 0.94× | ✅ |
| client | 18,483 | 19,757 | 0.9355× | 0.94× | ✅ |
| socket | 19,596 | 21,067 | 0.9301× | 0.92× | ✅ |
| gzip | 196,023 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,496 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,025 | 4,762 | 1.0552× | 1.05× | ❌ |
| tcp-eof | 5,567 | 10,231 | 0.5441× | 0.54× | ✅ |
| udp | 7,557 | 7,987 | 0.9462× | 1.0× | ✅ |
| connect | 18,280 | 18,707 | 0.9772× | 0.98× | ✅ |
| https | 108,807 | 108,810 | 1.0000× | 1.0× | ✅ |
| wss | 104,645 | 104,794 | 0.9986× | 1.0× | ✅ |
| atomic | 49,681 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,491 | 16,960 | 0.9723× | 0.97× | ✅ |
| http2-concurrent | 361,539 | 408,877 | 0.8842× | 0.89× | ✅ |
| websocket-64k | 156,178 | 159,691 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 12,887 | 16,331 | 0.7891× | 0.79× | ✅ |
| upload | 478,463 | 1,322,382 | 0.3618× | 0.42× | ✅ |
| static | 29,389 | 41,045 | 0.7160× | 0.84× | ✅ |
| pgsql | 23,224 | 24,594 | 0.9443× | 0.95× | ✅ |
| sqlite | 23,486 | 25,232 | 0.9308× | 0.94× | ✅ |
| curl | 85,300 | 90,317 | 0.9445× | 0.95× | ✅ |
| dns | 229,726 | 232,420 | 0.9884× | 1.0× | ✅ |

Geometric mean 0.8868×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
