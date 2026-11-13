# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d328f75` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,920 | 15,214 | 0.9150× | 0.92× | ✅ |
| echo-1k | 17,018 | 18,165 | 0.9369× | 0.94× | ✅ |
| json | 22,022 | 23,161 | 0.9508× | 0.95× | ✅ |
| service-health | 16,525 | 17,773 | 0.9297× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.82× | ✅ |
| timer | 203,125 | 313,570 | 0.6478× | 0.65× | ✅ |
| app | 125,785 | 135,825 | 0.9261× | 0.93× | ✅ |
| http2 | 18,653 | 20,464 | 0.9115× | 0.93× | ✅ |
| table | 195,481 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,413 | 154,654 | 0.8885× | 0.9× | ✅ |
| task | 32,098 | 33,261 | 0.9650× | 0.96× | ✅ |
| mysql | 49,574 | 51,058 | 0.9709× | 0.97× | ✅ |
| http-client | 33,761 | 35,805 | 0.9429× | 0.94× | ✅ |
| client | 18,483 | 19,757 | 0.9355× | 0.94× | ✅ |
| socket | 19,599 | 21,068 | 0.9303× | 0.92× | ✅ |
| gzip | 194,117 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,379 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,581 | 7,987 | 0.9491× | 0.95× | ✅ |
| connect | 18,331 | 18,707 | 0.9799× | 0.98× | ✅ |
| https | 108,796 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,472 | 104,797 | 0.9969× | 1.0× | ✅ |
| atomic | 49,688 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,675 | 16,960 | 0.9832× | 0.99× | ✅ |
| http2-concurrent | 359,245 | 408,877 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,692 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,331 | 0.7846× | 0.79× | ✅ |
| upload | 477,825 | 1,322,381 | 0.3613× | 0.37× | ✅ |
| static | 29,264 | 41,045 | 0.7130× | 0.72× | ✅ |
| pgsql | 23,199 | 24,594 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,482 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,753 | 90,465 | 0.9369× | 0.94× | ✅ |
| dns | 230,352 | 232,748 | 0.9897× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
