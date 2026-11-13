# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `479a50b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,910 | 15,213 | 0.9143× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,162 | 0.9383× | 0.94× | ✅ |
| json | 22,005 | 23,161 | 0.9501× | 0.95× | ✅ |
| service-health | 16,500 | 17,773 | 0.9283× | 0.93× | ✅ |
| websocket | 6,523 | 8,191 | 0.7964× | 0.81× | ✅ |
| timer | 203,236 | 313,567 | 0.6481× | 0.65× | ✅ |
| app | 125,375 | 135,973 | 0.9221× | 0.93× | ✅ |
| http2 | 18,565 | 20,463 | 0.9072× | 0.92× | ✅ |
| table | 195,474 | 201,869 | 0.9683× | 0.97× | ✅ |
| channel | 137,415 | 154,651 | 0.8886× | 0.89× | ✅ |
| task | 31,991 | 33,268 | 0.9616× | 0.96× | ✅ |
| mysql | 49,501 | 51,064 | 0.9694× | 0.97× | ✅ |
| http-client | 33,691 | 35,806 | 0.9409× | 0.94× | ✅ |
| client | 18,448 | 19,754 | 0.9339× | 0.94× | ✅ |
| socket | 19,579 | 21,067 | 0.9294× | 0.92× | ✅ |
| gzip | 194,109 | 197,079 | 0.9849× | 0.99× | ✅ |
| websocket-deflate | 179,309 | 181,568 | 0.9876× | 0.99× | ✅ |
| tcp-length | 4,738 | 4,762 | 0.9949× | 1.01× | ✅ |
| tcp-eof | 5,261 | 10,231 | 0.5142× | 0.52× | ✅ |
| udp | 7,531 | 7,987 | 0.9430× | 0.95× | ✅ |
| connect | 17,698 | 18,707 | 0.9461× | 0.98× | ✅ |
| https | 108,067 | 108,810 | 0.9932× | 1.0× | ✅ |
| wss | 103,674 | 104,797 | 0.9893× | 1.0× | ✅ |
| atomic | 49,617 | 51,316 | 0.9669× | 0.97× | ✅ |
| process-mode | 15,074 | 16,959 | 0.8888× | 0.94× | ✅ |
| http2-concurrent | 357,056 | 408,877 | 0.8733× | 0.88× | ✅ |
| websocket-64k | 155,967 | 159,694 | 0.9767× | 0.98× | ✅ |
| websocket-fragmented | 12,752 | 16,331 | 0.7809× | 0.79× | ✅ |
| upload | 477,872 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 27,364 | 41,000 | 0.6674× | 0.72× | ✅ |
| pgsql | 23,190 | 24,594 | 0.9429× | 0.95× | ✅ |
| sqlite | 23,451 | 25,236 | 0.9293× | 0.94× | ✅ |
| curl | 85,689 | 90,479 | 0.9471× | 0.94× | ✅ |
| dns | 230,990 | 233,079 | 0.9910× | 0.99× | ✅ |

Geometric mean 0.8748×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
