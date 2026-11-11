# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9db2eae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,990 | 15,213 | 0.9196× | 0.93× | ✅ |
| echo-1k | 17,112 | 18,166 | 0.9420× | 0.95× | ✅ |
| json | 22,078 | 23,161 | 0.9532× | 0.96× | ✅ |
| service-health | 16,573 | 17,777 | 0.9323× | 0.94× | ✅ |
| websocket | 6,656 | 8,188 | 0.8129× | 0.82× | ✅ |
| timer | 202,030 | 313,797 | 0.6438× | 0.65× | ✅ |
| app | 125,598 | 135,945 | 0.9239× | 0.93× | ✅ |
| http2 | 19,679 | 20,463 | 0.9617× | 0.98× | ✅ |
| table | 195,782 | 201,865 | 0.9699× | 0.97× | ✅ |
| channel | 138,076 | 154,654 | 0.8928× | 0.9× | ✅ |
| task | 31,988 | 33,268 | 0.9615× | 0.97× | ✅ |
| mysql | 49,469 | 51,064 | 0.9688× | 0.98× | ✅ |
| http-client | 33,441 | 35,808 | 0.9339× | 0.95× | ✅ |
| client | 18,460 | 19,757 | 0.9343× | 0.95× | ✅ |
| socket | 19,430 | 21,067 | 0.9223× | 0.94× | ✅ |
| gzip | 195,946 | 197,079 | 0.9943× | 1.0× | ✅ |
| websocket-deflate | 181,411 | 181,568 | 0.9991× | 1.0× | ✅ |
| tcp-length | 4,950 | 4,765 | 1.0387× | 1.09× | ➖ |
| tcp-eof | 5,518 | 10,231 | 0.5393× | 0.56× | ✅ |
| udp | 7,988 | 7,987 | 1.0002× | 1.0× | ❌ |
| connect | 18,250 | 18,707 | 0.9756× | 0.99× | ✅ |
| https | 108,845 | 108,810 | 1.0003× | 1.01× | ➖ |
| wss | 104,568 | 104,794 | 0.9978× | 1.0× | ✅ |
| atomic | 49,733 | 51,316 | 0.9692× | 0.98× | ✅ |
| process-mode | 16,445 | 16,956 | 0.9698× | 0.98× | ✅ |
| http2-concurrent | 371,526 | 408,877 | 0.9086× | 0.92× | ✅ |
| websocket-64k | 156,107 | 159,697 | 0.9775× | 0.98× | ✅ |
| websocket-fragmented | 12,983 | 16,331 | 0.7950× | 0.79× | ✅ |
| upload | 544,192 | 1,322,380 | 0.4115× | 0.42× | ✅ |
| static | 34,280 | 40,981 | 0.8365× | 0.84× | ✅ |
| pgsql | 23,228 | 24,595 | 0.9444× | 0.96× | ✅ |
| sqlite | 23,515 | 25,232 | 0.9320× | 0.95× | ✅ |
| curl | 85,215 | 90,568 | 0.9409× | 0.95× | ✅ |
| dns | 231,456 | 232,730 | 0.9945× | 1.0× | ✅ |

Geometric mean 0.8958×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
