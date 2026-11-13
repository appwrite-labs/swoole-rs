# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `63b587f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,966 | 15,210 | 0.9182× | 0.92× | ✅ |
| echo-1k | 17,072 | 18,166 | 0.9398× | 0.94× | ✅ |
| json | 22,046 | 23,161 | 0.9519× | 0.95× | ✅ |
| service-health | 16,544 | 17,777 | 0.9306× | 0.93× | ✅ |
| websocket | 6,580 | 8,188 | 0.8037× | 0.81× | ✅ |
| timer | 203,740 | 314,510 | 0.6478× | 0.65× | ✅ |
| app | 125,414 | 135,837 | 0.9233× | 0.93× | ✅ |
| http2 | 18,661 | 20,467 | 0.9118× | 0.92× | ✅ |
| table | 195,518 | 201,869 | 0.9685× | 0.97× | ✅ |
| channel | 137,459 | 154,654 | 0.8888× | 0.89× | ✅ |
| task | 32,014 | 33,268 | 0.9623× | 0.96× | ✅ |
| mysql | 49,559 | 51,062 | 0.9706× | 0.97× | ✅ |
| http-client | 33,744 | 35,805 | 0.9424× | 0.94× | ✅ |
| client | 18,496 | 19,757 | 0.9362× | 0.94× | ✅ |
| socket | 19,623 | 21,067 | 0.9315× | 0.92× | ✅ |
| gzip | 194,179 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,383 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,762 | 1.0060× | 1.01× | ➖ |
| tcp-eof | 5,313 | 10,231 | 0.5192× | 0.52× | ✅ |
| udp | 7,575 | 7,987 | 0.9485× | 0.95× | ✅ |
| connect | 18,403 | 18,707 | 0.9838× | 0.98× | ✅ |
| https | 108,783 | 108,813 | 0.9997× | 1.0× | ✅ |
| wss | 104,381 | 104,794 | 0.9961× | 1.0× | ✅ |
| atomic | 49,725 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,849 | 16,957 | 0.9347× | 0.94× | ✅ |
| http2-concurrent | 359,200 | 408,880 | 0.8785× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,796 | 16,331 | 0.7836× | 0.79× | ✅ |
| upload | 478,129 | 1,322,382 | 0.3616× | 0.37× | ✅ |
| static | 29,144 | 41,045 | 0.7100× | 0.72× | ✅ |
| pgsql | 23,247 | 24,595 | 0.9452× | 0.95× | ✅ |
| sqlite | 23,518 | 25,236 | 0.9319× | 0.94× | ✅ |
| curl | 85,463 | 90,350 | 0.9459× | 0.94× | ✅ |
| dns | 230,361 | 232,567 | 0.9905× | 0.99× | ✅ |

Geometric mean 0.8811×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
