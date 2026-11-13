# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b262d23` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,921 | 15,214 | 0.9151× | 0.92× | ✅ |
| echo-1k | 17,020 | 18,165 | 0.9369× | 0.94× | ✅ |
| json | 22,015 | 23,161 | 0.9505× | 0.95× | ✅ |
| service-health | 16,516 | 17,777 | 0.9291× | 0.93× | ✅ |
| websocket | 6,578 | 8,188 | 0.8034× | 0.82× | ✅ |
| timer | 203,624 | 313,566 | 0.6494× | 0.65× | ✅ |
| app | 125,810 | 135,915 | 0.9257× | 0.93× | ✅ |
| http2 | 18,643 | 20,468 | 0.9108× | 0.93× | ✅ |
| table | 195,469 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,437 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,096 | 33,268 | 0.9648× | 0.96× | ✅ |
| mysql | 49,589 | 51,008 | 0.9722× | 0.97× | ✅ |
| http-client | 33,744 | 35,805 | 0.9425× | 0.94× | ✅ |
| client | 18,475 | 19,757 | 0.9351× | 0.94× | ✅ |
| socket | 19,612 | 21,067 | 0.9310× | 0.92× | ✅ |
| gzip | 194,134 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,398 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0063× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,324 | 18,707 | 0.9796× | 0.98× | ✅ |
| https | 108,792 | 108,806 | 0.9999× | 1.0× | ✅ |
| wss | 104,477 | 104,803 | 0.9969× | 1.0× | ✅ |
| atomic | 49,680 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,678 | 17,024 | 0.9796× | 0.99× | ✅ |
| http2-concurrent | 360,011 | 408,877 | 0.8805× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,813 | 16,331 | 0.7846× | 0.79× | ✅ |
| upload | 477,918 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 29,280 | 41,074 | 0.7129× | 0.72× | ✅ |
| pgsql | 23,179 | 24,595 | 0.9425× | 0.95× | ✅ |
| sqlite | 23,468 | 25,221 | 0.9305× | 0.94× | ✅ |
| curl | 84,733 | 90,639 | 0.9348× | 0.94× | ✅ |
| dns | 230,623 | 232,733 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8822×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
