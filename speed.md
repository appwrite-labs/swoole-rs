# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e75c64f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,962 | 15,214 | 0.9177× | 0.92× | ✅ |
| echo-1k | 17,086 | 18,165 | 0.9406× | 0.94× | ✅ |
| json | 22,053 | 23,161 | 0.9522× | 0.95× | ✅ |
| service-health | 16,555 | 17,777 | 0.9313× | 0.93× | ✅ |
| websocket | 6,578 | 8,191 | 0.8030× | 0.81× | ✅ |
| timer | 203,491 | 313,592 | 0.6489× | 0.65× | ✅ |
| app | 125,430 | 135,938 | 0.9227× | 0.93× | ✅ |
| http2 | 18,661 | 20,464 | 0.9119× | 0.92× | ✅ |
| table | 195,533 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,474 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,045 | 33,268 | 0.9632× | 0.96× | ✅ |
| mysql | 49,557 | 51,064 | 0.9705× | 0.97× | ✅ |
| http-client | 33,727 | 35,803 | 0.9420× | 0.94× | ✅ |
| client | 18,507 | 19,757 | 0.9367× | 0.94× | ✅ |
| socket | 19,638 | 21,067 | 0.9322× | 0.92× | ✅ |
| gzip | 194,193 | 197,083 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,378 | 181,568 | 0.9879× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,762 | 1.0060× | 1.01× | ➖ |
| tcp-eof | 5,313 | 10,228 | 0.5194× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,442 | 18,707 | 0.9859× | 0.98× | ✅ |
| https | 108,809 | 108,810 | 1.0000× | 1.0× | ✅ |
| wss | 104,391 | 104,794 | 0.9962× | 1.0× | ✅ |
| atomic | 49,747 | 51,316 | 0.9694× | 0.97× | ✅ |
| process-mode | 15,859 | 16,957 | 0.9353× | 0.94× | ✅ |
| http2-concurrent | 359,132 | 408,881 | 0.8783× | 0.88× | ✅ |
| websocket-64k | 156,023 | 159,697 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,799 | 16,327 | 0.7839× | 0.79× | ✅ |
| upload | 478,226 | 1,322,380 | 0.3616× | 0.37× | ✅ |
| static | 29,144 | 41,045 | 0.7100× | 0.72× | ✅ |
| pgsql | 23,240 | 24,595 | 0.9449× | 0.95× | ✅ |
| sqlite | 23,511 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 85,453 | 90,351 | 0.9458× | 0.94× | ✅ |
| dns | 231,151 | 232,932 | 0.9924× | 0.99× | ✅ |

Geometric mean 0.8813×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
