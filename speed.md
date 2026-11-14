# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c50f5c9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,910 | 15,214 | 0.9143× | 0.92× | ✅ |
| echo-1k | 17,034 | 18,162 | 0.9379× | 0.94× | ✅ |
| json | 22,095 | 23,161 | 0.9540× | 0.95× | ✅ |
| service-health | 16,526 | 17,777 | 0.9296× | 0.93× | ✅ |
| websocket | 6,548 | 8,188 | 0.7998× | 0.8× | ✅ |
| timer | 207,155 | 313,546 | 0.6607× | 0.65× | ✅ |
| app | 125,526 | 135,912 | 0.9236× | 0.93× | ✅ |
| http2 | 18,666 | 20,464 | 0.9121× | 0.91× | ✅ |
| table | 195,983 | 201,865 | 0.9709× | 0.97× | ✅ |
| channel | 137,108 | 155,167 | 0.8836× | 0.89× | ✅ |
| task | 32,450 | 33,268 | 0.9754× | 0.96× | ✅ |
| mysql | 49,649 | 51,064 | 0.9723× | 0.97× | ✅ |
| http-client | 33,881 | 35,799 | 0.9464× | 0.94× | ✅ |
| client | 18,569 | 19,757 | 0.9399× | 0.94× | ✅ |
| socket | 19,697 | 21,072 | 0.9347× | 0.92× | ✅ |
| gzip | 194,141 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,012 | 181,572 | 0.9859× | 0.99× | ✅ |
| tcp-length | 4,709 | 4,765 | 0.9882× | 0.99× | ✅ |
| tcp-eof | 5,202 | 10,231 | 0.5084× | 0.51× | ✅ |
| udp | 7,503 | 7,987 | 0.9395× | 0.93× | ✅ |
| connect | 17,655 | 18,707 | 0.9438× | 0.94× | ✅ |
| https | 108,180 | 108,810 | 0.9942× | 1.0× | ✅ |
| wss | 103,790 | 104,794 | 0.9904× | 0.99× | ✅ |
| atomic | 49,679 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 15,035 | 16,960 | 0.8866× | 0.88× | ✅ |
| http2-concurrent | 360,520 | 409,073 | 0.8813× | 0.88× | ✅ |
| websocket-64k | 89,870 | 159,694 | 0.5628× | 0.57× | ✅ |
| websocket-fragmented | 11,986 | 16,328 | 0.7341× | 0.74× | ✅ |
| upload | 412,176 | 1,322,391 | 0.3117× | 0.32× | ✅ |
| static | 27,354 | 41,080 | 0.6659× | 0.66× | ✅ |
| pgsql | 23,325 | 24,595 | 0.9484× | 0.95× | ✅ |
| sqlite | 23,533 | 25,236 | 0.9325× | 0.93× | ✅ |
| curl | 86,094 | 90,258 | 0.9539× | 0.94× | ✅ |
| dns | 231,412 | 232,743 | 0.9943× | 0.99× | ✅ |

Geometric mean 0.8572×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
