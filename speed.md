# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9f077d8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,994 | 15,207 | 0.9203× | 0.93× | ✅ |
| echo-1k | 17,108 | 18,165 | 0.9418× | 0.95× | ✅ |
| json | 22,075 | 23,161 | 0.9531× | 0.96× | ✅ |
| service-health | 16,570 | 17,777 | 0.9321× | 0.94× | ✅ |
| websocket | 6,652 | 8,184 | 0.8128× | 0.82× | ✅ |
| timer | 202,056 | 313,571 | 0.6444× | 0.65× | ✅ |
| app | 125,617 | 135,970 | 0.9239× | 0.93× | ✅ |
| http2 | 19,681 | 20,468 | 0.9616× | 0.98× | ✅ |
| table | 195,782 | 201,865 | 0.9699× | 0.97× | ✅ |
| channel | 138,076 | 154,654 | 0.8928× | 0.9× | ✅ |
| task | 31,988 | 33,268 | 0.9615× | 0.97× | ✅ |
| mysql | 49,493 | 51,064 | 0.9692× | 0.98× | ✅ |
| http-client | 33,437 | 35,809 | 0.9338× | 0.95× | ✅ |
| client | 18,463 | 19,757 | 0.9345× | 0.95× | ✅ |
| socket | 19,429 | 21,067 | 0.9223× | 0.94× | ✅ |
| gzip | 195,950 | 197,083 | 0.9943× | 1.0× | ✅ |
| websocket-deflate | 181,414 | 181,572 | 0.9991× | 1.0× | ✅ |
| tcp-length | 4,950 | 4,763 | 1.0392× | 1.09× | ➖ |
| tcp-eof | 5,518 | 10,228 | 0.5395× | 0.56× | ✅ |
| udp | 7,989 | 7,987 | 1.0003× | 1.0× | ❌ |
| connect | 18,250 | 18,707 | 0.9756× | 0.99× | ✅ |
| https | 108,859 | 108,810 | 1.0005× | 1.01× | ➖ |
| wss | 104,575 | 104,794 | 0.9979× | 1.0× | ✅ |
| atomic | 49,733 | 51,316 | 0.9692× | 0.98× | ✅ |
| process-mode | 16,444 | 16,956 | 0.9698× | 0.98× | ✅ |
| http2-concurrent | 371,315 | 408,881 | 0.9081× | 0.92× | ✅ |
| websocket-64k | 156,098 | 159,693 | 0.9775× | 0.98× | ✅ |
| websocket-fragmented | 12,980 | 16,331 | 0.7948× | 0.79× | ✅ |
| upload | 544,179 | 1,322,380 | 0.4115× | 0.42× | ✅ |
| static | 34,292 | 40,981 | 0.8368× | 0.84× | ✅ |
| pgsql | 23,228 | 24,594 | 0.9445× | 0.96× | ✅ |
| sqlite | 23,515 | 25,228 | 0.9321× | 0.95× | ✅ |
| curl | 85,211 | 90,512 | 0.9414× | 0.95× | ✅ |
| dns | 231,508 | 233,054 | 0.9934× | 1.0× | ✅ |

Geometric mean 0.8958×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
