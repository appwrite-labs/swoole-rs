# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d8a0074` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,011 | 15,214 | 0.9210× | 0.93× | ✅ |
| echo-1k | 17,133 | 18,166 | 0.9431× | 0.95× | ✅ |
| json | 22,115 | 23,161 | 0.9548× | 0.96× | ✅ |
| service-health | 16,605 | 17,777 | 0.9341× | 0.94× | ✅ |
| websocket | 6,669 | 8,191 | 0.8142× | 0.82× | ✅ |
| timer | 202,193 | 313,569 | 0.6448× | 0.65× | ✅ |
| app | 125,689 | 136,026 | 0.9240× | 0.93× | ✅ |
| http2 | 19,747 | 20,464 | 0.9650× | 0.98× | ✅ |
| table | 195,821 | 201,865 | 0.9701× | 0.97× | ✅ |
| channel | 138,249 | 154,654 | 0.8939× | 0.9× | ✅ |
| task | 32,008 | 33,265 | 0.9622× | 0.97× | ✅ |
| mysql | 49,528 | 51,049 | 0.9702× | 0.98× | ✅ |
| http-client | 33,515 | 35,808 | 0.9360× | 0.95× | ✅ |
| client | 18,502 | 19,756 | 0.9365× | 0.95× | ✅ |
| socket | 19,471 | 21,068 | 0.9242× | 0.94× | ✅ |
| gzip | 195,982 | 197,079 | 0.9944× | 1.0× | ✅ |
| websocket-deflate | 181,425 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 5,170 | 4,762 | 1.0858× | 1.09× | ➖ |
| tcp-eof | 5,705 | 10,231 | 0.5576× | 0.56× | ✅ |
| udp | 7,918 | 7,986 | 0.9914× | 1.0× | ✅ |
| connect | 18,143 | 18,707 | 0.9699× | 0.99× | ✅ |
| https | 108,880 | 108,810 | 1.0006× | 1.01× | ➖ |
| wss | 104,583 | 104,794 | 0.9980× | 1.0× | ✅ |
| atomic | 49,769 | 51,316 | 0.9699× | 0.98× | ✅ |
| process-mode | 16,423 | 16,959 | 0.9684× | 0.98× | ✅ |
| http2-concurrent | 372,410 | 408,877 | 0.9108× | 0.92× | ✅ |
| websocket-64k | 156,116 | 159,692 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 12,715 | 16,331 | 0.7786× | 0.79× | ✅ |
| upload | 544,216 | 1,322,382 | 0.4115× | 0.42× | ✅ |
| static | 34,112 | 40,981 | 0.8324× | 0.84× | ✅ |
| pgsql | 23,268 | 24,598 | 0.9459× | 0.96× | ✅ |
| sqlite | 23,553 | 25,232 | 0.9335× | 0.95× | ✅ |
| curl | 85,251 | 90,507 | 0.9419× | 0.95× | ✅ |
| dns | 231,020 | 233,605 | 0.9889× | 1.0× | ✅ |

Geometric mean 0.8974×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
