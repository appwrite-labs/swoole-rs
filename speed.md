# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bf4e8ab` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,920 | 15,213 | 0.9150× | 0.92× | ✅ |
| echo-1k | 17,018 | 18,166 | 0.9368× | 0.94× | ✅ |
| json | 22,023 | 23,161 | 0.9508× | 0.95× | ✅ |
| service-health | 16,524 | 17,777 | 0.9295× | 0.93× | ✅ |
| websocket | 6,577 | 8,188 | 0.8032× | 0.82× | ✅ |
| timer | 203,299 | 313,545 | 0.6484× | 0.65× | ✅ |
| app | 125,746 | 135,952 | 0.9249× | 0.93× | ✅ |
| http2 | 18,651 | 20,468 | 0.9112× | 0.93× | ✅ |
| table | 195,481 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,413 | 154,654 | 0.8885× | 0.9× | ✅ |
| task | 32,095 | 33,265 | 0.9648× | 0.96× | ✅ |
| mysql | 49,581 | 51,061 | 0.9710× | 0.97× | ✅ |
| http-client | 33,762 | 35,808 | 0.9428× | 0.94× | ✅ |
| client | 18,483 | 19,757 | 0.9355× | 0.94× | ✅ |
| socket | 19,599 | 21,067 | 0.9303× | 0.92× | ✅ |
| gzip | 194,121 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,383 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,581 | 7,987 | 0.9492× | 0.95× | ✅ |
| connect | 18,331 | 18,703 | 0.9801× | 0.98× | ✅ |
| https | 108,813 | 108,810 | 1.0000× | 1.0× | ❌ |
| wss | 104,483 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,688 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,677 | 16,962 | 0.9832× | 0.99× | ✅ |
| http2-concurrent | 358,970 | 408,877 | 0.8779× | 0.88× | ✅ |
| websocket-64k | 156,023 | 159,699 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,331 | 0.7846× | 0.79× | ✅ |
| upload | 477,825 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 29,264 | 41,045 | 0.7130× | 0.72× | ✅ |
| pgsql | 23,195 | 24,600 | 0.9429× | 0.95× | ✅ |
| sqlite | 23,478 | 25,221 | 0.9309× | 0.94× | ✅ |
| curl | 84,745 | 90,482 | 0.9366× | 0.94× | ✅ |
| dns | 230,533 | 233,325 | 0.9880× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
