# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `40c49ab` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,008 | 15,213 | 0.9208× | 0.93× | ✅ |
| echo-1k | 17,133 | 18,165 | 0.9432× | 0.95× | ✅ |
| json | 22,118 | 23,161 | 0.9550× | 0.96× | ✅ |
| service-health | 16,605 | 17,777 | 0.9341× | 0.94× | ✅ |
| websocket | 6,669 | 8,188 | 0.8145× | 0.82× | ✅ |
| timer | 202,137 | 313,742 | 0.6443× | 0.65× | ✅ |
| app | 125,714 | 135,950 | 0.9247× | 0.93× | ✅ |
| http2 | 19,745 | 20,465 | 0.9648× | 0.98× | ✅ |
| table | 195,821 | 201,865 | 0.9701× | 0.97× | ✅ |
| channel | 138,249 | 154,654 | 0.8939× | 0.9× | ✅ |
| task | 32,017 | 33,264 | 0.9625× | 0.97× | ✅ |
| mysql | 49,532 | 51,061 | 0.9701× | 0.98× | ✅ |
| http-client | 33,533 | 35,809 | 0.9364× | 0.95× | ✅ |
| client | 18,505 | 19,757 | 0.9366× | 0.95× | ✅ |
| socket | 19,468 | 21,067 | 0.9241× | 0.94× | ✅ |
| gzip | 195,989 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,428 | 181,572 | 0.9992× | 1.0× | ✅ |
| tcp-length | 5,170 | 4,762 | 1.0858× | 1.09× | ➖ |
| tcp-eof | 5,705 | 10,228 | 0.5578× | 0.56× | ✅ |
| udp | 7,918 | 7,987 | 0.9914× | 1.0× | ✅ |
| connect | 18,136 | 18,707 | 0.9695× | 0.99× | ✅ |
| https | 108,880 | 108,810 | 1.0006× | 1.01× | ➖ |
| wss | 104,583 | 104,801 | 0.9979× | 1.0× | ✅ |
| atomic | 49,772 | 51,312 | 0.9700× | 0.98× | ✅ |
| process-mode | 16,421 | 16,959 | 0.9683× | 0.98× | ✅ |
| http2-concurrent | 372,613 | 408,882 | 0.9113× | 0.92× | ✅ |
| websocket-64k | 156,122 | 159,693 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 12,922 | 16,327 | 0.7914× | 0.79× | ✅ |
| upload | 544,226 | 1,322,380 | 0.4116× | 0.42× | ✅ |
| static | 34,094 | 40,981 | 0.8319× | 0.84× | ✅ |
| pgsql | 23,261 | 24,594 | 0.9458× | 0.96× | ✅ |
| sqlite | 23,553 | 25,232 | 0.9335× | 0.95× | ✅ |
| curl | 85,261 | 90,381 | 0.9433× | 0.95× | ✅ |
| dns | 231,312 | 232,748 | 0.9938× | 1.0× | ✅ |

Geometric mean 0.8980×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
