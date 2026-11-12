# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7b5b47e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,213 | 0.9154× | 0.92× | ✅ |
| echo-1k | 17,074 | 18,165 | 0.9399× | 0.94× | ✅ |
| json | 22,009 | 23,161 | 0.9502× | 0.96× | ✅ |
| service-health | 16,506 | 17,773 | 0.9287× | 0.93× | ✅ |
| websocket | 6,718 | 8,184 | 0.8208× | 0.82× | ✅ |
| timer | 202,419 | 313,617 | 0.6454× | 0.65× | ✅ |
| app | 125,918 | 135,896 | 0.9266× | 0.93× | ✅ |
| http2 | 19,776 | 20,463 | 0.9664× | 0.97× | ✅ |
| table | 195,719 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,800 | 154,654 | 0.8975× | 0.9× | ✅ |
| task | 31,972 | 33,268 | 0.9610× | 0.96× | ✅ |
| mysql | 49,565 | 51,053 | 0.9709× | 0.97× | ✅ |
| http-client | 33,582 | 35,803 | 0.9380× | 0.94× | ✅ |
| client | 18,459 | 19,756 | 0.9343× | 0.94× | ✅ |
| socket | 19,742 | 21,067 | 0.9371× | 0.92× | ✅ |
| gzip | 196,006 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,473 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,765 | 1.0538× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,228 | 0.5440× | 0.54× | ✅ |
| udp | 8,034 | 7,986 | 1.0059× | 1.0× | ❌ |
| connect | 18,254 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,817 | 108,803 | 1.0001× | 1.01× | ➖ |
| wss | 104,668 | 104,797 | 0.9988× | 1.0× | ✅ |
| atomic | 49,670 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,419 | 16,963 | 0.9679× | 0.97× | ✅ |
| http2-concurrent | 372,738 | 408,877 | 0.9116× | 0.91× | ✅ |
| websocket-64k | 156,162 | 159,693 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,069 | 16,331 | 0.8002× | 0.79× | ✅ |
| upload | 544,651 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,475 | 41,074 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,210 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,487 | 25,217 | 0.9314× | 0.94× | ✅ |
| curl | 85,227 | 90,316 | 0.9437× | 0.95× | ✅ |
| dns | 231,138 | 232,957 | 0.9922× | 1.0× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
