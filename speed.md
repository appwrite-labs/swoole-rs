# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `41f361b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,256 | 15,214 | 0.9371× | 0.93× | ✅ |
| echo-1k | 17,374 | 18,165 | 0.9565× | 0.95× | ✅ |
| json | 22,351 | 23,161 | 0.9650× | 0.96× | ✅ |
| service-health | 16,849 | 17,777 | 0.9478× | 0.94× | ✅ |
| websocket | 6,680 | 8,188 | 0.8158× | 0.83× | ✅ |
| timer | 202,173 | 313,587 | 0.6447× | 0.65× | ✅ |
| app | 126,458 | 135,917 | 0.9304× | 0.93× | ✅ |
| http2 | 20,022 | 20,467 | 0.9783× | 0.98× | ✅ |
| table | 196,093 | 201,865 | 0.9714× | 0.97× | ✅ |
| channel | 139,818 | 154,654 | 0.9041× | 0.91× | ✅ |
| task | 32,147 | 33,264 | 0.9664× | 0.97× | ✅ |
| mysql | 49,890 | 51,029 | 0.9777× | 0.98× | ✅ |
| http-client | 34,102 | 35,805 | 0.9524× | 0.95× | ✅ |
| client | 18,857 | 19,757 | 0.9545× | 0.95× | ✅ |
| socket | 19,833 | 21,067 | 0.9414× | 0.94× | ✅ |
| gzip | 196,231 | 197,083 | 0.9957× | 1.0× | ✅ |
| websocket-deflate | 181,445 | 181,572 | 0.9993× | 1.0× | ✅ |
| tcp-length | 5,168 | 4,763 | 1.0849× | 1.1× | ➖ |
| tcp-eof | 5,699 | 10,228 | 0.5571× | 0.57× | ✅ |
| udp | 7,953 | 7,986 | 0.9958× | 1.0× | ✅ |
| connect | 18,421 | 18,707 | 0.9847× | 0.99× | ✅ |
| https | 109,170 | 108,803 | 1.0034× | 1.01× | ➖ |
| wss | 104,585 | 104,794 | 0.9980× | 1.0× | ✅ |
| atomic | 50,048 | 51,316 | 0.9753× | 0.98× | ✅ |
| process-mode | 16,658 | 16,963 | 0.9820× | 0.98× | ✅ |
| http2-concurrent | 375,846 | 408,883 | 0.9192× | 0.92× | ✅ |
| websocket-64k | 156,137 | 159,696 | 0.9777× | 0.98× | ✅ |
| websocket-fragmented | 12,926 | 16,331 | 0.7915× | 0.79× | ✅ |
| upload | 544,490 | 1,322,386 | 0.4117× | 0.42× | ✅ |
| static | 34,196 | 41,010 | 0.8338× | 0.85× | ✅ |
| pgsql | 23,681 | 24,594 | 0.9629× | 0.96× | ✅ |
| sqlite | 23,914 | 25,232 | 0.9478× | 0.95× | ✅ |
| curl | 85,718 | 90,513 | 0.9470× | 0.95× | ✅ |
| dns | 231,613 | 232,745 | 0.9951× | 1.0× | ✅ |

Geometric mean 0.9046×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
