# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b345729` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,912 | 15,213 | 0.9145× | 0.92× | ✅ |
| echo-1k | 17,025 | 18,165 | 0.9372× | 0.94× | ✅ |
| json | 22,013 | 23,161 | 0.9504× | 0.95× | ✅ |
| service-health | 16,515 | 17,777 | 0.9290× | 0.93× | ✅ |
| websocket | 6,577 | 8,188 | 0.8033× | 0.82× | ✅ |
| timer | 203,235 | 313,613 | 0.6480× | 0.65× | ✅ |
| app | 125,797 | 135,930 | 0.9255× | 0.93× | ✅ |
| http2 | 18,668 | 20,464 | 0.9122× | 0.93× | ✅ |
| table | 195,468 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,395 | 154,654 | 0.8884× | 0.9× | ✅ |
| task | 32,156 | 33,261 | 0.9668× | 0.96× | ✅ |
| mysql | 49,558 | 51,061 | 0.9706× | 0.97× | ✅ |
| http-client | 33,630 | 35,808 | 0.9392× | 0.94× | ✅ |
| client | 18,473 | 19,757 | 0.9350× | 0.94× | ✅ |
| socket | 19,586 | 21,063 | 0.9299× | 0.92× | ✅ |
| gzip | 194,113 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,379 | 181,568 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,762 | 1.0054× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,231 | 0.5215× | 0.54× | ✅ |
| udp | 7,577 | 7,987 | 0.9487× | 0.95× | ✅ |
| connect | 18,352 | 18,707 | 0.9811× | 0.98× | ✅ |
| https | 108,806 | 108,810 | 1.0000× | 1.0× | ✅ |
| wss | 104,482 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,675 | 51,312 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,670 | 16,959 | 0.9829× | 0.99× | ✅ |
| http2-concurrent | 359,185 | 408,881 | 0.8785× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,808 | 16,327 | 0.7845× | 0.79× | ✅ |
| upload | 478,487 | 1,322,380 | 0.3618× | 0.37× | ✅ |
| static | 29,188 | 40,981 | 0.7122× | 0.72× | ✅ |
| pgsql | 23,205 | 24,595 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,480 | 25,236 | 0.9304× | 0.94× | ✅ |
| curl | 84,759 | 90,350 | 0.9381× | 0.94× | ✅ |
| dns | 230,542 | 232,714 | 0.9907× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
