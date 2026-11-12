# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `802351e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,942 | 15,214 | 0.9164× | 0.92× | ✅ |
| echo-1k | 17,045 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,044 | 23,161 | 0.9518× | 0.95× | ✅ |
| service-health | 16,539 | 17,777 | 0.9304× | 0.93× | ✅ |
| websocket | 6,575 | 8,188 | 0.8030× | 0.82× | ✅ |
| timer | 203,383 | 313,541 | 0.6487× | 0.65× | ✅ |
| app | 125,834 | 135,954 | 0.9256× | 0.93× | ✅ |
| http2 | 18,667 | 20,464 | 0.9122× | 0.93× | ✅ |
| table | 195,746 | 201,865 | 0.9697× | 0.97× | ✅ |
| channel | 138,894 | 154,651 | 0.8981× | 0.9× | ✅ |
| task | 32,163 | 33,268 | 0.9668× | 0.96× | ✅ |
| mysql | 49,615 | 51,064 | 0.9716× | 0.97× | ✅ |
| http-client | 33,719 | 35,805 | 0.9417× | 0.94× | ✅ |
| client | 18,495 | 19,757 | 0.9361× | 0.94× | ✅ |
| socket | 19,614 | 21,067 | 0.9311× | 0.92× | ✅ |
| gzip | 196,028 | 197,083 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,339 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,773 | 4,762 | 1.0022× | 1.05× | ➖ |
| tcp-eof | 5,321 | 10,231 | 0.5200× | 0.54× | ✅ |
| udp | 7,584 | 7,987 | 0.9496× | 0.95× | ✅ |
| connect | 18,371 | 18,707 | 0.9821× | 0.98× | ✅ |
| https | 108,818 | 108,810 | 1.0001× | 1.0× | ❌ |
| wss | 104,469 | 104,797 | 0.9969× | 1.0× | ✅ |
| atomic | 49,697 | 51,316 | 0.9685× | 0.97× | ✅ |
| process-mode | 16,718 | 16,956 | 0.9860× | 0.99× | ✅ |
| http2-concurrent | 359,470 | 408,881 | 0.8792× | 0.88× | ✅ |
| websocket-64k | 156,025 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,810 | 16,331 | 0.7844× | 0.79× | ✅ |
| upload | 478,434 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,181 | 40,981 | 0.7121× | 0.72× | ✅ |
| pgsql | 23,218 | 24,595 | 0.9440× | 0.95× | ✅ |
| sqlite | 23,500 | 25,221 | 0.9317× | 0.94× | ✅ |
| curl | 84,600 | 90,354 | 0.9363× | 0.94× | ✅ |
| dns | 230,611 | 232,727 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8832×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
