# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9f91f67` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,818 | 15,214 | 0.9083× | 0.92× | ✅ |
| echo-1k | 16,927 | 18,165 | 0.9318× | 0.94× | ✅ |
| json | 21,904 | 23,161 | 0.9457× | 0.95× | ✅ |
| service-health | 16,405 | 17,777 | 0.9228× | 0.93× | ✅ |
| websocket | 6,473 | 8,188 | 0.7906× | 0.8× | ✅ |
| timer | 203,142 | 313,609 | 0.6478× | 0.65× | ✅ |
| app | 125,273 | 135,947 | 0.9215× | 0.93× | ✅ |
| http2 | 18,512 | 20,468 | 0.9044× | 0.91× | ✅ |
| table | 195,485 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,361 | 155,170 | 0.8852× | 0.89× | ✅ |
| task | 31,888 | 33,261 | 0.9587× | 0.96× | ✅ |
| mysql | 49,412 | 51,061 | 0.9677× | 0.97× | ✅ |
| http-client | 33,648 | 35,802 | 0.9398× | 0.94× | ✅ |
| client | 18,406 | 19,756 | 0.9316× | 0.94× | ✅ |
| socket | 19,533 | 21,072 | 0.9270× | 0.92× | ✅ |
| gzip | 194,029 | 197,083 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 179,291 | 181,572 | 0.9874× | 0.99× | ✅ |
| tcp-length | 4,685 | 4,762 | 0.9838× | 0.99× | ✅ |
| tcp-eof | 5,208 | 10,231 | 0.5090× | 0.51× | ✅ |
| udp | 7,380 | 7,986 | 0.9241× | 0.93× | ✅ |
| connect | 17,518 | 18,707 | 0.9365× | 0.94× | ✅ |
| https | 108,025 | 108,810 | 0.9928× | 1.0× | ✅ |
| wss | 103,652 | 104,801 | 0.9890× | 0.99× | ✅ |
| atomic | 49,557 | 51,316 | 0.9657× | 0.97× | ✅ |
| process-mode | 14,911 | 16,957 | 0.8794× | 0.88× | ✅ |
| http2-concurrent | 357,085 | 408,882 | 0.8733× | 0.88× | ✅ |
| websocket-64k | 155,922 | 159,696 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,703 | 16,327 | 0.7780× | 0.78× | ✅ |
| upload | 478,003 | 1,322,387 | 0.3615× | 0.37× | ✅ |
| static | 26,888 | 40,981 | 0.6561× | 0.67× | ✅ |
| pgsql | 23,216 | 24,596 | 0.9439× | 0.95× | ✅ |
| sqlite | 23,430 | 25,236 | 0.9284× | 0.93× | ✅ |
| curl | 85,778 | 90,350 | 0.9494× | 0.94× | ✅ |
| dns | 230,702 | 232,624 | 0.9917× | 0.99× | ✅ |

Geometric mean 0.8714×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
