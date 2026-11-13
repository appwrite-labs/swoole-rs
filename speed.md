# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c0d902e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,982 | 15,214 | 0.9191× | 0.92× | ✅ |
| echo-1k | 17,097 | 18,162 | 0.9414× | 0.94× | ✅ |
| json | 22,085 | 23,161 | 0.9535× | 0.95× | ✅ |
| service-health | 16,581 | 17,777 | 0.9327× | 0.93× | ✅ |
| websocket | 6,578 | 8,191 | 0.8030× | 0.81× | ✅ |
| timer | 203,461 | 313,546 | 0.6489× | 0.65× | ✅ |
| app | 125,427 | 135,919 | 0.9228× | 0.93× | ✅ |
| http2 | 18,672 | 20,468 | 0.9122× | 0.92× | ✅ |
| table | 195,533 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,474 | 154,655 | 0.8889× | 0.89× | ✅ |
| task | 32,056 | 33,265 | 0.9637× | 0.96× | ✅ |
| mysql | 49,619 | 51,061 | 0.9718× | 0.97× | ✅ |
| http-client | 33,743 | 35,804 | 0.9424× | 0.94× | ✅ |
| client | 18,516 | 19,754 | 0.9373× | 0.94× | ✅ |
| socket | 19,643 | 21,067 | 0.9324× | 0.92× | ✅ |
| gzip | 194,180 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,387 | 181,572 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,765 | 1.0053× | 1.01× | ➖ |
| tcp-eof | 5,313 | 10,231 | 0.5192× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,425 | 18,707 | 0.9849× | 0.98× | ✅ |
| https | 108,803 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,393 | 104,794 | 0.9962× | 1.0× | ✅ |
| atomic | 49,740 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,874 | 16,959 | 0.9360× | 0.94× | ✅ |
| http2-concurrent | 359,516 | 408,881 | 0.8793× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,800 | 16,327 | 0.7840× | 0.79× | ✅ |
| upload | 478,102 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,159 | 41,045 | 0.7104× | 0.72× | ✅ |
| pgsql | 23,242 | 24,595 | 0.9450× | 0.95× | ✅ |
| sqlite | 23,511 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 84,885 | 90,393 | 0.9391× | 0.94× | ✅ |
| dns | 231,086 | 232,518 | 0.9938× | 0.99× | ✅ |

Geometric mean 0.8814×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
