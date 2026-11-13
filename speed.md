# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `93a547b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,973 | 15,210 | 0.9187× | 0.92× | ✅ |
| echo-1k | 17,079 | 18,165 | 0.9402× | 0.94× | ✅ |
| json | 22,069 | 23,161 | 0.9529× | 0.95× | ✅ |
| service-health | 16,560 | 17,773 | 0.9317× | 0.93× | ✅ |
| websocket | 6,578 | 8,191 | 0.8030× | 0.81× | ✅ |
| timer | 203,108 | 313,537 | 0.6478× | 0.65× | ✅ |
| app | 125,410 | 135,930 | 0.9226× | 0.93× | ✅ |
| http2 | 18,635 | 20,464 | 0.9106× | 0.92× | ✅ |
| table | 195,534 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,472 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,015 | 33,265 | 0.9624× | 0.96× | ✅ |
| mysql | 49,570 | 51,061 | 0.9708× | 0.97× | ✅ |
| http-client | 33,733 | 35,805 | 0.9421× | 0.94× | ✅ |
| client | 18,511 | 19,757 | 0.9370× | 0.94× | ✅ |
| socket | 19,639 | 21,067 | 0.9322× | 0.92× | ✅ |
| gzip | 194,178 | 197,083 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,367 | 181,572 | 0.9879× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,762 | 1.0060× | 1.01× | ➖ |
| tcp-eof | 5,314 | 10,231 | 0.5194× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,410 | 18,707 | 0.9842× | 0.98× | ✅ |
| https | 108,792 | 108,810 | 0.9998× | 1.0× | ✅ |
| wss | 104,395 | 104,797 | 0.9962× | 1.0× | ✅ |
| atomic | 49,741 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,865 | 17,024 | 0.9319× | 0.94× | ✅ |
| http2-concurrent | 359,260 | 409,439 | 0.8774× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,807 | 16,331 | 0.7842× | 0.79× | ✅ |
| upload | 477,917 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 29,183 | 41,074 | 0.7105× | 0.72× | ✅ |
| pgsql | 23,239 | 24,595 | 0.9449× | 0.95× | ✅ |
| sqlite | 23,510 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 85,751 | 90,326 | 0.9493× | 0.94× | ✅ |
| dns | 231,023 | 233,061 | 0.9913× | 0.99× | ✅ |

Geometric mean 0.8812×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
