# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `03f2399` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,305 | 15,214 | 0.9403× | 0.93× | ✅ |
| echo-1k | 17,420 | 18,165 | 0.9590× | 0.95× | ✅ |
| json | 22,414 | 23,162 | 0.9677× | 0.96× | ✅ |
| service-health | 16,912 | 17,777 | 0.9514× | 0.94× | ✅ |
| websocket | 6,731 | 8,184 | 0.8224× | 0.83× | ✅ |
| timer | 202,268 | 313,779 | 0.6446× | 0.65× | ✅ |
| app | 126,535 | 135,828 | 0.9316× | 0.93× | ✅ |
| http2 | 20,129 | 20,464 | 0.9836× | 0.98× | ✅ |
| table | 196,160 | 201,869 | 0.9717× | 0.97× | ✅ |
| channel | 139,368 | 154,651 | 0.9012× | 0.91× | ✅ |
| task | 32,236 | 33,263 | 0.9691× | 0.97× | ✅ |
| mysql | 49,951 | 51,052 | 0.9784× | 0.98× | ✅ |
| http-client | 34,161 | 35,809 | 0.9540× | 0.95× | ✅ |
| client | 18,921 | 19,757 | 0.9577× | 0.95× | ✅ |
| socket | 19,894 | 21,067 | 0.9443× | 0.94× | ✅ |
| gzip | 196,280 | 197,083 | 0.9959× | 1.0× | ✅ |
| websocket-deflate | 181,497 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,214 | 4,762 | 1.0948× | 1.1× | ➖ |
| tcp-eof | 5,745 | 10,231 | 0.5615× | 0.57× | ✅ |
| udp | 7,974 | 7,987 | 0.9984× | 1.0× | ✅ |
| connect | 18,536 | 18,707 | 0.9908× | 0.99× | ✅ |
| https | 109,268 | 108,810 | 1.0042× | 1.01× | ➖ |
| wss | 104,692 | 104,800 | 0.9990× | 1.0× | ✅ |
| atomic | 50,111 | 51,316 | 0.9765× | 0.98× | ✅ |
| process-mode | 16,760 | 16,959 | 0.9882× | 0.98× | ✅ |
| http2-concurrent | 377,123 | 408,881 | 0.9223× | 0.92× | ✅ |
| websocket-64k | 156,179 | 159,693 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 12,786 | 16,327 | 0.7831× | 0.79× | ✅ |
| upload | 544,556 | 1,322,382 | 0.4118× | 0.42× | ✅ |
| static | 34,305 | 40,981 | 0.8371× | 0.85× | ✅ |
| pgsql | 23,726 | 24,595 | 0.9647× | 0.96× | ✅ |
| sqlite | 23,960 | 25,232 | 0.9496× | 0.95× | ✅ |
| curl | 85,769 | 90,390 | 0.9489× | 0.95× | ✅ |
| dns | 231,746 | 233,135 | 0.9940× | 1.0× | ✅ |

Geometric mean 0.9066×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
