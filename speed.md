# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `13a9d71` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,972 | 15,213 | 0.9184× | 0.92× | ✅ |
| echo-1k | 17,086 | 18,165 | 0.9406× | 0.94× | ✅ |
| json | 22,078 | 23,158 | 0.9534× | 0.95× | ✅ |
| service-health | 16,577 | 17,777 | 0.9325× | 0.93× | ✅ |
| websocket | 6,580 | 8,188 | 0.8037× | 0.81× | ✅ |
| timer | 203,464 | 313,602 | 0.6488× | 0.65× | ✅ |
| app | 125,369 | 135,952 | 0.9222× | 0.93× | ✅ |
| http2 | 18,678 | 20,468 | 0.9125× | 0.92× | ✅ |
| table | 195,526 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,464 | 154,654 | 0.8888× | 0.89× | ✅ |
| task | 32,030 | 33,265 | 0.9629× | 0.96× | ✅ |
| mysql | 49,552 | 51,064 | 0.9704× | 0.97× | ✅ |
| http-client | 33,693 | 35,809 | 0.9409× | 0.94× | ✅ |
| client | 18,484 | 19,757 | 0.9356× | 0.94× | ✅ |
| socket | 19,611 | 21,067 | 0.9309× | 0.92× | ✅ |
| gzip | 194,173 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,382 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,795 | 4,762 | 1.0069× | 1.01× | ➖ |
| tcp-eof | 5,308 | 10,231 | 0.5188× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,418 | 18,707 | 0.9846× | 0.98× | ✅ |
| https | 108,811 | 108,813 | 1.0000× | 1.0× | ✅ |
| wss | 104,383 | 104,794 | 0.9961× | 1.0× | ✅ |
| atomic | 49,733 | 51,316 | 0.9692× | 0.97× | ✅ |
| process-mode | 15,868 | 17,022 | 0.9322× | 0.99× | ✅ |
| http2-concurrent | 359,759 | 408,881 | 0.8799× | 0.88× | ✅ |
| websocket-64k | 156,020 | 159,691 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,804 | 16,327 | 0.7842× | 0.79× | ✅ |
| upload | 478,100 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,185 | 41,045 | 0.7110× | 0.72× | ✅ |
| pgsql | 23,237 | 24,594 | 0.9448× | 0.95× | ✅ |
| sqlite | 23,504 | 25,236 | 0.9314× | 0.94× | ✅ |
| curl | 84,876 | 90,320 | 0.9397× | 0.94× | ✅ |
| dns | 231,046 | 232,477 | 0.9938× | 0.99× | ✅ |

Geometric mean 0.8811×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
