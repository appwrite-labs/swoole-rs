# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,214 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,106 | 23,161 | 0.9544× | 0.95× | ✅ |
| service-health | 16,545 | 17,777 | 0.9307× | 0.93× | ✅ |
| websocket | 6,632 | 8,188 | 0.8100× | 0.8× | ✅ |
| timer | 205,900 | 313,611 | 0.6565× | 0.65× | ✅ |
| app | 125,401 | 135,914 | 0.9226× | 0.93× | ✅ |
| http2 | 18,692 | 20,469 | 0.9132× | 0.91× | ✅ |
| table | 195,560 | 201,865 | 0.9688× | 0.97× | ✅ |
| channel | 136,581 | 155,174 | 0.8802× | 0.88× | ✅ |
| task | 32,510 | 33,265 | 0.9773× | 0.96× | ✅ |
| mysql | 49,469 | 51,062 | 0.9688× | 0.97× | ✅ |
| http-client | 33,818 | 35,797 | 0.9447× | 0.94× | ✅ |
| client | 18,556 | 19,757 | 0.9392× | 0.94× | ✅ |
| socket | 19,677 | 21,073 | 0.9338× | 0.92× | ✅ |
| gzip | 194,174 | 197,083 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,094 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,228 | 0.5094× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9444× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,199 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,855 | 104,800 | 0.9910× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,098 | 16,957 | 0.8904× | 0.88× | ✅ |
| http2-concurrent | 361,648 | 409,075 | 0.8841× | 0.88× | ✅ |
| websocket-64k | 89,958 | 159,692 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,331 | 0.7269× | 0.73× | ✅ |
| upload | 413,440 | 1,322,387 | 0.3126× | 0.32× | ✅ |
| static | 27,325 | 41,051 | 0.6656× | 0.66× | ✅ |
| pgsql | 23,374 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,533 | 25,236 | 0.9325× | 0.93× | ✅ |
| curl | 86,120 | 90,479 | 0.9518× | 0.94× | ✅ |
| dns | 231,072 | 233,181 | 0.9910× | 0.99× | ✅ |

Geometric mean 0.8576×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
