# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9bfe166` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,809 | 15,213 | 0.9077× | 0.92× | ✅ |
| echo-1k | 16,905 | 18,166 | 0.9306× | 0.94× | ✅ |
| json | 21,895 | 23,161 | 0.9453× | 0.95× | ✅ |
| service-health | 16,399 | 17,777 | 0.9225× | 0.93× | ✅ |
| websocket | 6,471 | 8,188 | 0.7904× | 0.8× | ✅ |
| timer | 203,158 | 313,460 | 0.6481× | 0.65× | ✅ |
| app | 125,344 | 135,974 | 0.9218× | 0.93× | ✅ |
| http2 | 18,538 | 20,464 | 0.9059× | 0.91× | ✅ |
| table | 195,028 | 201,865 | 0.9661× | 0.97× | ✅ |
| channel | 137,871 | 155,171 | 0.8885× | 0.89× | ✅ |
| task | 31,892 | 33,268 | 0.9587× | 0.96× | ✅ |
| mysql | 49,453 | 51,044 | 0.9688× | 0.97× | ✅ |
| http-client | 33,656 | 35,800 | 0.9401× | 0.94× | ✅ |
| client | 18,407 | 19,757 | 0.9317× | 0.94× | ✅ |
| socket | 19,540 | 21,072 | 0.9273× | 0.92× | ✅ |
| gzip | 194,004 | 197,079 | 0.9844× | 0.99× | ✅ |
| websocket-deflate | 179,289 | 181,560 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,684 | 4,765 | 0.9829× | 0.99× | ✅ |
| tcp-eof | 5,207 | 10,231 | 0.5089× | 0.51× | ✅ |
| udp | 7,379 | 7,986 | 0.9240× | 0.93× | ✅ |
| connect | 17,528 | 18,707 | 0.9370× | 0.94× | ✅ |
| https | 108,047 | 108,810 | 0.9930× | 1.0× | ✅ |
| wss | 103,678 | 104,794 | 0.9893× | 0.99× | ✅ |
| atomic | 49,562 | 51,312 | 0.9659× | 0.97× | ✅ |
| process-mode | 14,892 | 16,956 | 0.8783× | 0.88× | ✅ |
| http2-concurrent | 356,901 | 409,069 | 0.8725× | 0.88× | ✅ |
| websocket-64k | 155,913 | 159,695 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,697 | 16,331 | 0.7775× | 0.78× | ✅ |
| upload | 478,021 | 1,322,388 | 0.3615× | 0.37× | ✅ |
| static | 26,973 | 41,016 | 0.6576× | 0.67× | ✅ |
| pgsql | 23,217 | 24,594 | 0.9440× | 0.95× | ✅ |
| sqlite | 23,428 | 25,236 | 0.9283× | 0.93× | ✅ |
| curl | 85,783 | 90,350 | 0.9494× | 0.94× | ✅ |
| dns | 230,099 | 232,720 | 0.9887× | 0.99× | ✅ |

Geometric mean 0.8714×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
