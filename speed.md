# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `920af87` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,916 | 15,210 | 0.9149× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,027 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,529 | 17,777 | 0.9298× | 0.93× | ✅ |
| websocket | 6,573 | 8,191 | 0.8025× | 0.82× | ✅ |
| timer | 203,218 | 313,870 | 0.6475× | 0.65× | ✅ |
| app | 125,760 | 135,891 | 0.9255× | 0.93× | ✅ |
| http2 | 18,647 | 20,465 | 0.9112× | 0.93× | ✅ |
| table | 195,486 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,413 | 154,651 | 0.8885× | 0.9× | ✅ |
| task | 32,173 | 33,264 | 0.9672× | 0.96× | ✅ |
| mysql | 49,573 | 51,011 | 0.9718× | 0.97× | ✅ |
| http-client | 33,763 | 35,810 | 0.9428× | 0.94× | ✅ |
| client | 18,488 | 19,754 | 0.9359× | 0.94× | ✅ |
| socket | 19,604 | 21,067 | 0.9306× | 0.92× | ✅ |
| gzip | 194,113 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,375 | 181,568 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,762 | 1.0054× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,228 | 0.5217× | 0.54× | ✅ |
| udp | 7,577 | 7,987 | 0.9487× | 0.95× | ✅ |
| connect | 18,341 | 18,707 | 0.9804× | 0.98× | ✅ |
| https | 108,797 | 108,806 | 0.9999× | 1.0× | ✅ |
| wss | 104,482 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,693 | 51,312 | 0.9684× | 0.97× | ✅ |
| process-mode | 16,672 | 16,959 | 0.9831× | 0.99× | ✅ |
| http2-concurrent | 355,806 | 408,882 | 0.8702× | 0.88× | ✅ |
| websocket-64k | 156,020 | 159,697 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,327 | 0.7847× | 0.79× | ✅ |
| upload | 478,690 | 1,322,380 | 0.3620× | 0.37× | ✅ |
| static | 29,261 | 41,071 | 0.7124× | 0.72× | ✅ |
| pgsql | 23,210 | 24,595 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,489 | 25,236 | 0.9308× | 0.94× | ✅ |
| curl | 84,765 | 90,468 | 0.9370× | 0.94× | ✅ |
| dns | 229,790 | 232,588 | 0.9880× | 0.99× | ✅ |

Geometric mean 0.8819×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
