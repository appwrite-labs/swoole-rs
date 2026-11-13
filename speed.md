# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5ac76fc` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,921 | 15,214 | 0.9151× | 0.92× | ✅ |
| echo-1k | 17,019 | 18,165 | 0.9369× | 0.94× | ✅ |
| json | 22,033 | 23,161 | 0.9513× | 0.95× | ✅ |
| service-health | 16,541 | 17,777 | 0.9305× | 0.93× | ✅ |
| websocket | 6,577 | 8,191 | 0.8030× | 0.82× | ✅ |
| timer | 203,476 | 313,572 | 0.6489× | 0.65× | ✅ |
| app | 125,760 | 135,773 | 0.9263× | 0.93× | ✅ |
| http2 | 18,648 | 20,463 | 0.9113× | 0.93× | ✅ |
| table | 195,491 | 201,869 | 0.9684× | 0.97× | ✅ |
| channel | 138,458 | 154,651 | 0.8953× | 0.9× | ✅ |
| task | 32,095 | 33,265 | 0.9649× | 0.96× | ✅ |
| mysql | 49,587 | 51,064 | 0.9711× | 0.97× | ✅ |
| http-client | 33,772 | 35,805 | 0.9432× | 0.94× | ✅ |
| client | 18,500 | 19,757 | 0.9364× | 0.94× | ✅ |
| socket | 19,637 | 21,067 | 0.9321× | 0.92× | ✅ |
| gzip | 194,134 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,421 | 181,572 | 0.9882× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,228 | 0.5221× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,325 | 18,707 | 0.9796× | 0.98× | ✅ |
| https | 108,799 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,477 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,698 | 51,316 | 0.9685× | 0.97× | ✅ |
| process-mode | 16,680 | 16,960 | 0.9835× | 0.99× | ✅ |
| http2-concurrent | 358,938 | 408,882 | 0.8779× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 13,027 | 16,327 | 0.7979× | 0.79× | ✅ |
| upload | 477,974 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 29,282 | 41,045 | 0.7134× | 0.72× | ✅ |
| pgsql | 23,203 | 24,595 | 0.9434× | 0.95× | ✅ |
| sqlite | 23,481 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,753 | 90,354 | 0.9380× | 0.94× | ✅ |
| dns | 231,200 | 232,868 | 0.9928× | 0.99× | ✅ |

Geometric mean 0.8829×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
