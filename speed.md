# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `41225db` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,966 | 15,210 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,072 | 18,162 | 0.9400× | 0.94× | ✅ |
| json | 22,046 | 23,161 | 0.9519× | 0.95× | ✅ |
| service-health | 16,544 | 17,773 | 0.9308× | 0.93× | ✅ |
| websocket | 6,578 | 8,191 | 0.8030× | 0.81× | ✅ |
| timer | 203,483 | 313,533 | 0.6490× | 0.65× | ✅ |
| app | 125,435 | 135,941 | 0.9227× | 0.93× | ✅ |
| http2 | 18,652 | 20,464 | 0.9115× | 0.92× | ✅ |
| table | 195,518 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,459 | 154,654 | 0.8888× | 0.89× | ✅ |
| task | 32,025 | 33,264 | 0.9628× | 0.96× | ✅ |
| mysql | 49,562 | 51,061 | 0.9706× | 0.97× | ✅ |
| http-client | 33,717 | 35,809 | 0.9416× | 0.94× | ✅ |
| client | 18,496 | 19,757 | 0.9361× | 0.94× | ✅ |
| socket | 19,623 | 21,067 | 0.9315× | 0.92× | ✅ |
| gzip | 194,179 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,378 | 181,568 | 0.9879× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,762 | 1.0060× | 1.01× | ➖ |
| tcp-eof | 5,313 | 10,231 | 0.5192× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,403 | 18,707 | 0.9838× | 0.98× | ✅ |
| https | 108,798 | 108,806 | 0.9999× | 1.0× | ✅ |
| wss | 104,385 | 104,800 | 0.9960× | 1.0× | ✅ |
| atomic | 49,725 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,849 | 17,021 | 0.9312× | 0.94× | ✅ |
| http2-concurrent | 359,221 | 408,877 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,693 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,796 | 16,331 | 0.7836× | 0.79× | ✅ |
| upload | 478,129 | 1,322,388 | 0.3616× | 0.37× | ✅ |
| static | 29,144 | 41,045 | 0.7100× | 0.72× | ✅ |
| pgsql | 23,247 | 24,595 | 0.9452× | 0.95× | ✅ |
| sqlite | 23,518 | 25,236 | 0.9319× | 0.94× | ✅ |
| curl | 85,463 | 90,334 | 0.9461× | 0.94× | ✅ |
| dns | 231,025 | 233,441 | 0.9897× | 0.99× | ✅ |

Geometric mean 0.8810×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
