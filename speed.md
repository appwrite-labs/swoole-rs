# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2038504` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,210 | 0.9156× | 0.92× | ✅ |
| echo-1k | 17,075 | 18,162 | 0.9401× | 0.94× | ✅ |
| json | 22,008 | 23,161 | 0.9502× | 0.96× | ✅ |
| service-health | 16,506 | 17,773 | 0.9287× | 0.93× | ✅ |
| websocket | 6,714 | 8,191 | 0.8196× | 0.82× | ✅ |
| timer | 202,447 | 313,503 | 0.6458× | 0.65× | ✅ |
| app | 125,934 | 135,981 | 0.9261× | 0.93× | ✅ |
| http2 | 19,770 | 20,463 | 0.9661× | 0.97× | ✅ |
| table | 195,719 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,803 | 154,651 | 0.8975× | 0.9× | ✅ |
| task | 31,968 | 33,268 | 0.9609× | 0.96× | ✅ |
| mysql | 49,468 | 51,061 | 0.9688× | 0.97× | ✅ |
| http-client | 33,583 | 35,801 | 0.9381× | 0.94× | ✅ |
| client | 18,462 | 19,757 | 0.9345× | 0.94× | ✅ |
| socket | 19,742 | 21,067 | 0.9371× | 0.92× | ✅ |
| gzip | 196,006 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,473 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,762 | 1.0545× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,228 | 0.5440× | 0.54× | ✅ |
| udp | 8,033 | 7,987 | 1.0059× | 1.0× | ❌ |
| connect | 18,251 | 18,707 | 0.9756× | 0.98× | ✅ |
| https | 108,831 | 108,813 | 1.0002× | 1.01× | ➖ |
| wss | 104,678 | 104,797 | 0.9989× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,420 | 16,957 | 0.9683× | 0.97× | ✅ |
| http2-concurrent | 372,729 | 408,880 | 0.9116× | 0.91× | ✅ |
| websocket-64k | 156,160 | 159,696 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,071 | 16,327 | 0.8006× | 0.79× | ✅ |
| upload | 544,652 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,475 | 41,074 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,210 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,487 | 25,232 | 0.9309× | 0.94× | ✅ |
| curl | 85,240 | 90,380 | 0.9431× | 0.95× | ✅ |
| dns | 230,844 | 233,032 | 0.9906× | 1.0× | ✅ |

Geometric mean 0.8976×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
