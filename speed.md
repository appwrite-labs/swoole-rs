# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1f51609` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,939 | 15,214 | 0.9162× | 0.92× | ✅ |
| echo-1k | 17,040 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,027 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,528 | 17,777 | 0.9298× | 0.93× | ✅ |
| websocket | 6,581 | 8,188 | 0.8037× | 0.82× | ✅ |
| timer | 203,742 | 313,609 | 0.6497× | 0.65× | ✅ |
| app | 125,500 | 135,936 | 0.9232× | 0.93× | ✅ |
| http2 | 18,683 | 20,467 | 0.9128× | 0.93× | ✅ |
| table | 195,462 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,430 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,119 | 33,264 | 0.9656× | 0.96× | ✅ |
| mysql | 49,630 | 51,062 | 0.9720× | 0.97× | ✅ |
| http-client | 33,764 | 35,806 | 0.9430× | 0.94× | ✅ |
| client | 18,465 | 19,757 | 0.9346× | 0.94× | ✅ |
| socket | 19,606 | 21,067 | 0.9306× | 0.92× | ✅ |
| gzip | 194,139 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,397 | 181,568 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0056× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,228 | 0.5221× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,352 | 18,707 | 0.9811× | 0.98× | ✅ |
| https | 108,795 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,481 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,669 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,689 | 17,024 | 0.9803× | 0.99× | ✅ |
| http2-concurrent | 359,172 | 408,880 | 0.8784× | 0.88× | ✅ |
| websocket-64k | 156,026 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,829 | 16,327 | 0.7857× | 0.79× | ✅ |
| upload | 477,963 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 29,275 | 41,045 | 0.7132× | 0.72× | ✅ |
| pgsql | 23,196 | 24,593 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,479 | 25,236 | 0.9304× | 0.94× | ✅ |
| curl | 84,745 | 90,477 | 0.9366× | 0.94× | ✅ |
| dns | 230,271 | 232,961 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8823×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
