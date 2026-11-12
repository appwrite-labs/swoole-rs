# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `288a9db` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,995 | 15,214 | 0.9199× | 0.92× | ✅ |
| echo-1k | 17,115 | 18,165 | 0.9422× | 0.94× | ✅ |
| json | 22,103 | 23,161 | 0.9543× | 0.95× | ✅ |
| service-health | 16,602 | 17,777 | 0.9339× | 0.93× | ✅ |
| websocket | 6,727 | 8,191 | 0.8212× | 0.82× | ✅ |
| timer | 203,050 | 313,586 | 0.6475× | 0.65× | ✅ |
| app | 125,929 | 135,855 | 0.9269× | 0.93× | ✅ |
| http2 | 19,274 | 20,464 | 0.9418× | 0.94× | ✅ |
| table | 195,801 | 201,865 | 0.9700× | 0.97× | ✅ |
| channel | 138,438 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,027 | 33,265 | 0.9628× | 0.96× | ✅ |
| mysql | 49,641 | 51,061 | 0.9722× | 0.97× | ✅ |
| http-client | 33,705 | 35,809 | 0.9413× | 0.94× | ✅ |
| client | 18,549 | 19,757 | 0.9389× | 0.94× | ✅ |
| socket | 19,664 | 21,067 | 0.9334× | 0.92× | ✅ |
| gzip | 196,071 | 197,079 | 0.9949× | 1.0× | ✅ |
| websocket-deflate | 181,487 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 4,998 | 4,762 | 1.0495× | 1.05× | ➖ |
| tcp-eof | 5,533 | 10,231 | 0.5408× | 0.54× | ✅ |
| udp | 7,544 | 7,987 | 0.9446× | 0.95× | ✅ |
| connect | 18,317 | 18,707 | 0.9792× | 0.98× | ✅ |
| https | 108,851 | 108,803 | 1.0004× | 1.0× | ❌ |
| wss | 104,606 | 104,801 | 0.9981× | 1.0× | ✅ |
| atomic | 49,752 | 51,316 | 0.9695× | 0.97× | ✅ |
| process-mode | 16,558 | 16,963 | 0.9762× | 0.97× | ✅ |
| http2-concurrent | 361,671 | 408,877 | 0.8845× | 0.89× | ✅ |
| websocket-64k | 156,175 | 159,697 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,096 | 16,327 | 0.8021× | 0.79× | ✅ |
| upload | 478,509 | 1,322,382 | 0.3619× | 0.42× | ✅ |
| static | 29,313 | 41,016 | 0.7147× | 0.84× | ✅ |
| pgsql | 23,284 | 24,594 | 0.9467× | 0.95× | ✅ |
| sqlite | 23,551 | 25,232 | 0.9334× | 0.94× | ✅ |
| curl | 85,064 | 90,350 | 0.9415× | 0.95× | ✅ |
| dns | 230,387 | 233,232 | 0.9878× | 0.99× | ✅ |

Geometric mean 0.8878×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
