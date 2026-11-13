# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a4dddbb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,972 | 15,213 | 0.9184× | 0.92× | ✅ |
| echo-1k | 17,086 | 18,165 | 0.9406× | 0.94× | ✅ |
| json | 22,075 | 23,161 | 0.9531× | 0.95× | ✅ |
| service-health | 16,574 | 17,777 | 0.9323× | 0.93× | ✅ |
| websocket | 6,577 | 8,184 | 0.8036× | 0.81× | ✅ |
| timer | 203,451 | 313,593 | 0.6488× | 0.65× | ✅ |
| app | 125,352 | 135,999 | 0.9217× | 0.93× | ✅ |
| http2 | 18,679 | 20,468 | 0.9126× | 0.92× | ✅ |
| table | 195,526 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,464 | 154,654 | 0.8888× | 0.89× | ✅ |
| task | 32,031 | 33,261 | 0.9630× | 0.96× | ✅ |
| mysql | 49,554 | 51,064 | 0.9704× | 0.97× | ✅ |
| http-client | 33,692 | 35,802 | 0.9411× | 0.94× | ✅ |
| client | 18,480 | 19,757 | 0.9354× | 0.94× | ✅ |
| socket | 19,611 | 21,067 | 0.9309× | 0.92× | ✅ |
| gzip | 194,173 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,382 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,795 | 4,762 | 1.0069× | 1.01× | ➖ |
| tcp-eof | 5,308 | 10,231 | 0.5188× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,422 | 18,707 | 0.9848× | 0.98× | ✅ |
| https | 108,811 | 108,810 | 1.0000× | 1.0× | ❌ |
| wss | 104,377 | 104,794 | 0.9960× | 1.0× | ✅ |
| atomic | 49,733 | 51,316 | 0.9692× | 0.97× | ✅ |
| process-mode | 15,871 | 16,956 | 0.9360× | 0.99× | ✅ |
| http2-concurrent | 359,696 | 408,880 | 0.8797× | 0.88× | ✅ |
| websocket-64k | 156,018 | 159,697 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,808 | 16,327 | 0.7845× | 0.79× | ✅ |
| upload | 478,099 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,243 | 41,103 | 0.7114× | 0.72× | ✅ |
| pgsql | 23,237 | 24,594 | 0.9448× | 0.95× | ✅ |
| sqlite | 23,504 | 25,221 | 0.9319× | 0.94× | ✅ |
| curl | 84,881 | 90,482 | 0.9381× | 0.94× | ✅ |
| dns | 231,132 | 233,059 | 0.9917× | 0.99× | ✅ |

Geometric mean 0.8811×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
