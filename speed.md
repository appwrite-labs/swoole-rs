# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `661954e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,214 | 0.9154× | 0.92× | ✅ |
| echo-1k | 17,040 | 18,165 | 0.9380× | 0.94× | ✅ |
| json | 22,002 | 23,161 | 0.9500× | 0.96× | ✅ |
| service-health | 16,510 | 17,777 | 0.9287× | 0.93× | ✅ |
| websocket | 6,714 | 8,188 | 0.8201× | 0.82× | ✅ |
| timer | 202,485 | 313,836 | 0.6452× | 0.65× | ✅ |
| app | 125,922 | 135,881 | 0.9267× | 0.93× | ✅ |
| http2 | 19,813 | 20,464 | 0.9682× | 0.97× | ✅ |
| table | 195,719 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,803 | 154,651 | 0.8975× | 0.9× | ✅ |
| task | 31,950 | 33,268 | 0.9604× | 0.96× | ✅ |
| mysql | 49,551 | 51,021 | 0.9712× | 0.97× | ✅ |
| http-client | 33,575 | 35,808 | 0.9376× | 0.94× | ✅ |
| client | 18,459 | 19,757 | 0.9343× | 0.94× | ✅ |
| socket | 19,422 | 21,068 | 0.9219× | 0.92× | ✅ |
| gzip | 196,002 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,473 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,762 | 1.0545× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,231 | 0.5438× | 0.54× | ✅ |
| udp | 8,033 | 7,986 | 1.0059× | 1.0× | ❌ |
| connect | 18,261 | 18,707 | 0.9762× | 0.98× | ✅ |
| https | 108,817 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,668 | 104,801 | 0.9987× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,423 | 16,959 | 0.9684× | 0.97× | ✅ |
| http2-concurrent | 371,937 | 409,234 | 0.9089× | 0.92× | ✅ |
| websocket-64k | 156,160 | 159,691 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,069 | 16,328 | 0.8004× | 0.79× | ✅ |
| upload | 544,649 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,487 | 41,074 | 0.8396× | 0.84× | ✅ |
| pgsql | 23,200 | 24,594 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,479 | 25,232 | 0.9305× | 0.94× | ✅ |
| curl | 85,224 | 90,507 | 0.9416× | 0.95× | ✅ |
| dns | 230,938 | 232,633 | 0.9927× | 1.0× | ✅ |

Geometric mean 0.8971×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
