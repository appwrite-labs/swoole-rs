# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `34ee01d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,982 | 15,214 | 0.9190× | 0.92× | ✅ |
| echo-1k | 17,073 | 18,165 | 0.9398× | 0.94× | ✅ |
| json | 22,128 | 23,161 | 0.9554× | 0.95× | ✅ |
| service-health | 16,564 | 17,777 | 0.9318× | 0.93× | ✅ |
| websocket | 6,616 | 8,188 | 0.8081× | 0.8× | ✅ |
| timer | 206,146 | 313,641 | 0.6573× | 0.65× | ✅ |
| app | 125,360 | 135,890 | 0.9225× | 0.93× | ✅ |
| http2 | 18,755 | 20,468 | 0.9163× | 0.91× | ✅ |
| table | 195,131 | 201,865 | 0.9666× | 0.97× | ✅ |
| channel | 136,759 | 155,170 | 0.8813× | 0.88× | ✅ |
| task | 32,515 | 33,268 | 0.9774× | 0.96× | ✅ |
| mysql | 49,570 | 51,047 | 0.9711× | 0.97× | ✅ |
| http-client | 33,870 | 35,797 | 0.9462× | 0.94× | ✅ |
| client | 18,561 | 19,757 | 0.9395× | 0.94× | ✅ |
| socket | 19,699 | 21,072 | 0.9348× | 0.92× | ✅ |
| gzip | 194,188 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,070 | 181,572 | 0.9862× | 0.99× | ✅ |
| tcp-length | 4,703 | 4,762 | 0.9876× | 0.99× | ✅ |
| tcp-eof | 5,198 | 10,231 | 0.5080× | 0.51× | ✅ |
| udp | 7,489 | 7,986 | 0.9378× | 0.93× | ✅ |
| connect | 17,739 | 18,707 | 0.9482× | 0.94× | ✅ |
| https | 108,210 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,807 | 104,794 | 0.9906× | 0.99× | ✅ |
| atomic | 49,794 | 51,316 | 0.9703× | 0.97× | ✅ |
| process-mode | 15,094 | 17,021 | 0.8868× | 0.88× | ✅ |
| http2-concurrent | 362,338 | 409,073 | 0.8858× | 0.88× | ✅ |
| websocket-64k | 89,941 | 159,696 | 0.5632× | 0.57× | ✅ |
| websocket-fragmented | 11,868 | 16,331 | 0.7267× | 0.73× | ✅ |
| upload | 413,570 | 1,322,392 | 0.3127× | 0.32× | ✅ |
| static | 27,469 | 40,987 | 0.6702× | 0.66× | ✅ |
| pgsql | 23,380 | 24,594 | 0.9506× | 0.95× | ✅ |
| sqlite | 23,560 | 25,236 | 0.9336× | 0.93× | ✅ |
| curl | 86,211 | 90,354 | 0.9542× | 0.94× | ✅ |
| dns | 231,379 | 232,607 | 0.9947× | 0.99× | ✅ |

Geometric mean 0.8579×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
