# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `beee920` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,924 | 15,213 | 0.9153× | 0.92× | ✅ |
| echo-1k | 17,025 | 18,165 | 0.9372× | 0.94× | ✅ |
| json | 22,008 | 23,161 | 0.9502× | 0.95× | ✅ |
| service-health | 16,509 | 17,777 | 0.9287× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.82× | ✅ |
| timer | 203,624 | 313,631 | 0.6492× | 0.65× | ✅ |
| app | 125,721 | 135,901 | 0.9251× | 0.93× | ✅ |
| http2 | 18,649 | 20,464 | 0.9113× | 0.93× | ✅ |
| table | 195,466 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,433 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,097 | 33,264 | 0.9649× | 0.96× | ✅ |
| mysql | 49,510 | 51,061 | 0.9696× | 0.97× | ✅ |
| http-client | 33,689 | 35,804 | 0.9409× | 0.94× | ✅ |
| client | 18,435 | 19,756 | 0.9331× | 0.94× | ✅ |
| socket | 19,578 | 21,067 | 0.9293× | 0.92× | ✅ |
| gzip | 194,120 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,397 | 181,568 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,763 | 1.0061× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,351 | 18,707 | 0.9810× | 0.98× | ✅ |
| https | 108,791 | 108,803 | 0.9999× | 1.0× | ✅ |
| wss | 104,481 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,669 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,668 | 16,963 | 0.9826× | 0.99× | ✅ |
| http2-concurrent | 359,079 | 409,578 | 0.8767× | 0.88× | ✅ |
| websocket-64k | 156,028 | 159,700 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,825 | 16,331 | 0.7853× | 0.79× | ✅ |
| upload | 478,009 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,278 | 41,045 | 0.7133× | 0.72× | ✅ |
| pgsql | 23,195 | 24,593 | 0.9431× | 0.95× | ✅ |
| sqlite | 23,485 | 25,221 | 0.9312× | 0.94× | ✅ |
| curl | 84,844 | 90,482 | 0.9377× | 0.94× | ✅ |
| dns | 230,317 | 232,930 | 0.9888× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
