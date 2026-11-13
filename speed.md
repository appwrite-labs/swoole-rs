# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `78131f9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,869 | 15,210 | 0.9118× | 0.92× | ✅ |
| echo-1k | 16,969 | 18,162 | 0.9344× | 0.94× | ✅ |
| json | 21,962 | 23,158 | 0.9484× | 0.95× | ✅ |
| service-health | 16,455 | 17,777 | 0.9257× | 0.93× | ✅ |
| websocket | 6,480 | 8,188 | 0.7914× | 0.81× | ✅ |
| timer | 203,017 | 313,623 | 0.6473× | 0.65× | ✅ |
| app | 125,354 | 135,878 | 0.9225× | 0.93× | ✅ |
| http2 | 18,481 | 20,464 | 0.9031× | 0.92× | ✅ |
| table | 195,429 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,367 | 155,167 | 0.8853× | 0.89× | ✅ |
| task | 31,912 | 33,268 | 0.9592× | 0.96× | ✅ |
| mysql | 49,447 | 51,064 | 0.9683× | 0.97× | ✅ |
| http-client | 33,647 | 35,804 | 0.9398× | 0.94× | ✅ |
| client | 18,434 | 19,756 | 0.9330× | 0.94× | ✅ |
| socket | 19,562 | 21,067 | 0.9286× | 0.92× | ✅ |
| gzip | 194,068 | 197,083 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,261 | 181,572 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,762 | 0.9849× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,228 | 0.5097× | 0.52× | ✅ |
| udp | 7,375 | 7,987 | 0.9235× | 0.95× | ✅ |
| connect | 17,543 | 18,707 | 0.9378× | 0.98× | ✅ |
| https | 108,038 | 108,813 | 0.9929× | 1.0× | ✅ |
| wss | 103,624 | 104,800 | 0.9888× | 1.0× | ✅ |
| atomic | 49,572 | 51,312 | 0.9661× | 0.97× | ✅ |
| process-mode | 15,021 | 16,960 | 0.8857× | 0.94× | ✅ |
| http2-concurrent | 357,242 | 409,074 | 0.8733× | 0.88× | ✅ |
| websocket-64k | 155,920 | 159,698 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,708 | 16,327 | 0.7784× | 0.79× | ✅ |
| upload | 477,878 | 1,322,389 | 0.3614× | 0.37× | ✅ |
| static | 27,278 | 40,987 | 0.6655× | 0.72× | ✅ |
| pgsql | 23,173 | 24,595 | 0.9422× | 0.95× | ✅ |
| sqlite | 23,437 | 25,236 | 0.9287× | 0.94× | ✅ |
| curl | 85,680 | 90,350 | 0.9483× | 0.94× | ✅ |
| dns | 231,200 | 233,274 | 0.9911× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
