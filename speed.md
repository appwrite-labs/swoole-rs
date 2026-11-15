# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,214 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,165 | 0.9394× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,773 | 0.9319× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 205,687 | 313,559 | 0.6560× | 0.65× | ✅ |
| app | 125,359 | 135,877 | 0.9226× | 0.93× | ✅ |
| http2 | 18,703 | 20,465 | 0.9139× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,087 | 155,167 | 0.8770× | 0.88× | ✅ |
| task | 32,507 | 33,264 | 0.9772× | 0.96× | ✅ |
| mysql | 49,576 | 51,058 | 0.9710× | 0.97× | ✅ |
| http-client | 33,795 | 35,794 | 0.9441× | 0.94× | ✅ |
| client | 18,534 | 19,757 | 0.9381× | 0.94× | ✅ |
| socket | 19,665 | 21,072 | 0.9332× | 0.92× | ✅ |
| gzip | 194,182 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,090 | 181,560 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9444× | 0.93× | ✅ |
| connect | 17,745 | 18,703 | 0.9487× | 0.94× | ✅ |
| https | 108,202 | 108,813 | 0.9944× | 1.0× | ✅ |
| wss | 103,851 | 104,794 | 0.9910× | 0.99× | ✅ |
| atomic | 49,724 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 15,094 | 16,959 | 0.8900× | 0.88× | ✅ |
| http2-concurrent | 362,273 | 409,069 | 0.8856× | 0.88× | ✅ |
| websocket-64k | 89,954 | 159,690 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,870 | 16,331 | 0.7268× | 0.73× | ✅ |
| upload | 413,455 | 1,322,387 | 0.3127× | 0.32× | ✅ |
| static | 27,342 | 41,080 | 0.6656× | 0.66× | ✅ |
| pgsql | 23,375 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,544 | 25,236 | 0.9329× | 0.93× | ✅ |
| curl | 86,053 | 90,354 | 0.9524× | 0.94× | ✅ |
| dns | 231,500 | 233,068 | 0.9933× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
