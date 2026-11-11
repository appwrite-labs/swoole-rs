# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fdeada4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,234 | 15,210 | 0.9358× | 0.93× | ✅ |
| echo-1k | 17,360 | 18,165 | 0.9557× | 0.95× | ✅ |
| json | 22,335 | 23,161 | 0.9643× | 0.96× | ✅ |
| service-health | 16,832 | 17,777 | 0.9469× | 0.94× | ✅ |
| websocket | 6,765 | 8,188 | 0.8262× | 0.82× | ✅ |
| timer | 202,056 | 313,826 | 0.6438× | 0.65× | ✅ |
| app | 126,348 | 135,936 | 0.9295× | 0.93× | ✅ |
| http2 | 20,041 | 20,464 | 0.9793× | 0.98× | ✅ |
| table | 196,021 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 138,449 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,244 | 33,264 | 0.9693× | 0.97× | ✅ |
| mysql | 49,877 | 51,059 | 0.9769× | 0.98× | ✅ |
| http-client | 33,825 | 35,809 | 0.9446× | 0.95× | ✅ |
| client | 18,770 | 19,757 | 0.9501× | 0.95× | ✅ |
| socket | 19,737 | 21,067 | 0.9369× | 0.94× | ✅ |
| gzip | 196,202 | 197,083 | 0.9955× | 1.0× | ✅ |
| websocket-deflate | 181,521 | 181,568 | 0.9997× | 1.0× | ✅ |
| tcp-length | 5,259 | 4,762 | 1.1043× | 1.09× | ❌ |
| tcp-eof | 5,794 | 10,228 | 0.5664× | 0.56× | ✅ |
| udp | 7,997 | 7,987 | 1.0012× | 1.0× | ❌ |
| connect | 18,469 | 18,707 | 0.9873× | 0.99× | ✅ |
| https | 109,133 | 108,810 | 1.0030× | 1.01× | ➖ |
| wss | 104,744 | 104,803 | 0.9994× | 1.0× | ✅ |
| atomic | 49,976 | 51,316 | 0.9739× | 0.98× | ✅ |
| process-mode | 16,634 | 16,957 | 0.9810× | 0.98× | ✅ |
| http2-concurrent | 375,431 | 408,995 | 0.9179× | 0.92× | ✅ |
| websocket-64k | 156,224 | 159,699 | 0.9782× | 0.98× | ✅ |
| websocket-fragmented | 13,026 | 16,331 | 0.7976× | 0.79× | ✅ |
| upload | 544,397 | 1,322,380 | 0.4117× | 0.42× | ✅ |
| static | 34,372 | 41,039 | 0.8375× | 0.84× | ✅ |
| pgsql | 23,608 | 24,594 | 0.9599× | 0.96× | ✅ |
| sqlite | 23,835 | 25,232 | 0.9446× | 0.95× | ✅ |
| curl | 85,650 | 90,385 | 0.9476× | 0.95× | ✅ |
| dns | 231,370 | 232,976 | 0.9931× | 1.0× | ✅ |

Geometric mean 0.9053×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
