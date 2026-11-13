# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0258d76` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,810 | 15,210 | 0.9079× | 0.92× | ✅ |
| echo-1k | 16,914 | 18,165 | 0.9311× | 0.94× | ✅ |
| json | 21,902 | 23,161 | 0.9456× | 0.95× | ✅ |
| service-health | 16,397 | 17,777 | 0.9224× | 0.93× | ✅ |
| websocket | 6,470 | 8,188 | 0.7902× | 0.8× | ✅ |
| timer | 204,696 | 313,518 | 0.6529× | 0.65× | ✅ |
| app | 125,462 | 135,913 | 0.9231× | 0.93× | ✅ |
| http2 | 18,499 | 20,468 | 0.9038× | 0.91× | ✅ |
| table | 195,034 | 201,865 | 0.9662× | 0.97× | ✅ |
| channel | 137,875 | 155,174 | 0.8885× | 0.89× | ✅ |
| task | 31,862 | 33,265 | 0.9578× | 0.96× | ✅ |
| mysql | 49,391 | 51,060 | 0.9673× | 0.97× | ✅ |
| http-client | 33,723 | 35,803 | 0.9419× | 0.94× | ✅ |
| client | 18,444 | 19,757 | 0.9335× | 0.94× | ✅ |
| socket | 19,575 | 21,073 | 0.9289× | 0.92× | ✅ |
| gzip | 194,023 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 179,287 | 181,568 | 0.9874× | 0.99× | ✅ |
| tcp-length | 4,682 | 4,762 | 0.9832× | 0.99× | ✅ |
| tcp-eof | 5,175 | 10,228 | 0.5060× | 0.51× | ✅ |
| udp | 7,374 | 7,986 | 0.9233× | 0.93× | ✅ |
| connect | 17,576 | 18,707 | 0.9396× | 0.94× | ✅ |
| https | 108,043 | 108,810 | 0.9930× | 1.0× | ✅ |
| wss | 103,685 | 104,800 | 0.9894× | 0.99× | ✅ |
| atomic | 49,573 | 51,316 | 0.9660× | 0.97× | ✅ |
| process-mode | 14,889 | 17,021 | 0.8748× | 0.88× | ✅ |
| http2-concurrent | 357,628 | 409,073 | 0.8742× | 0.88× | ✅ |
| websocket-64k | 155,908 | 159,695 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,699 | 16,328 | 0.7778× | 0.78× | ✅ |
| upload | 478,014 | 1,322,387 | 0.3615× | 0.37× | ✅ |
| static | 27,032 | 41,016 | 0.6591× | 0.67× | ✅ |
| pgsql | 23,240 | 24,594 | 0.9450× | 0.95× | ✅ |
| sqlite | 23,417 | 25,236 | 0.9279× | 0.93× | ✅ |
| curl | 85,826 | 90,367 | 0.9497× | 0.94× | ✅ |
| dns | 230,696 | 232,775 | 0.9911× | 0.99× | ✅ |

Geometric mean 0.8717×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
