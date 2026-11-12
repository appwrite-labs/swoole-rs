# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7292c4d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,958 | 15,214 | 0.9175× | 0.92× | ✅ |
| echo-1k | 17,076 | 18,165 | 0.9400× | 0.94× | ✅ |
| json | 22,054 | 23,161 | 0.9522× | 0.96× | ✅ |
| service-health | 16,556 | 17,777 | 0.9313× | 0.93× | ✅ |
| websocket | 6,714 | 8,188 | 0.8201× | 0.82× | ✅ |
| timer | 202,582 | 313,623 | 0.6459× | 0.65× | ✅ |
| app | 126,007 | 135,971 | 0.9267× | 0.93× | ✅ |
| http2 | 19,839 | 20,464 | 0.9695× | 0.97× | ✅ |
| table | 195,768 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,852 | 154,651 | 0.8978× | 0.9× | ✅ |
| task | 31,989 | 33,265 | 0.9616× | 0.96× | ✅ |
| mysql | 49,604 | 51,064 | 0.9714× | 0.97× | ✅ |
| http-client | 33,596 | 35,806 | 0.9383× | 0.94× | ✅ |
| client | 18,508 | 19,756 | 0.9368× | 0.94× | ✅ |
| socket | 19,471 | 21,067 | 0.9243× | 0.92× | ✅ |
| gzip | 195,923 | 197,083 | 0.9941× | 1.0× | ✅ |
| websocket-deflate | 181,473 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,762 | 1.0545× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,231 | 0.5438× | 0.54× | ✅ |
| udp | 8,034 | 7,987 | 1.0059× | 1.0× | ❌ |
| connect | 18,264 | 18,707 | 0.9764× | 0.98× | ✅ |
| https | 108,852 | 108,813 | 1.0004× | 1.01× | ➖ |
| wss | 104,678 | 104,797 | 0.9989× | 1.0× | ✅ |
| atomic | 49,719 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 16,468 | 16,959 | 0.9710× | 0.97× | ✅ |
| http2-concurrent | 372,851 | 408,881 | 0.9119× | 0.92× | ✅ |
| websocket-64k | 156,165 | 159,699 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,071 | 16,331 | 0.8004× | 0.79× | ✅ |
| upload | 544,680 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,487 | 41,045 | 0.8402× | 0.84× | ✅ |
| pgsql | 23,246 | 24,595 | 0.9451× | 0.95× | ✅ |
| sqlite | 23,523 | 25,232 | 0.9323× | 0.94× | ✅ |
| curl | 85,270 | 90,496 | 0.9423× | 0.95× | ✅ |
| dns | 231,148 | 233,010 | 0.9920× | 1.0× | ✅ |

Geometric mean 0.8979×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
