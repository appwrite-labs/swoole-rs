# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3bcbd0c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,946 | 15,210 | 0.9169× | 0.92× | ✅ |
| echo-1k | 17,049 | 18,165 | 0.9385× | 0.94× | ✅ |
| json | 22,054 | 23,161 | 0.9522× | 0.95× | ✅ |
| service-health | 16,559 | 17,773 | 0.9317× | 0.93× | ✅ |
| websocket | 6,560 | 8,191 | 0.8009× | 0.81× | ✅ |
| timer | 203,124 | 313,654 | 0.6476× | 0.65× | ✅ |
| app | 125,462 | 135,909 | 0.9231× | 0.93× | ✅ |
| http2 | 18,607 | 20,464 | 0.9093× | 0.92× | ✅ |
| table | 195,533 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,471 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 31,997 | 33,264 | 0.9619× | 0.96× | ✅ |
| mysql | 49,533 | 51,061 | 0.9701× | 0.97× | ✅ |
| http-client | 33,720 | 35,806 | 0.9417× | 0.94× | ✅ |
| client | 18,507 | 19,756 | 0.9367× | 0.94× | ✅ |
| socket | 19,639 | 21,067 | 0.9322× | 0.92× | ✅ |
| gzip | 194,147 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,342 | 181,568 | 0.9877× | 0.99× | ✅ |
| tcp-length | 4,771 | 4,765 | 1.0012× | 1.01× | ➖ |
| tcp-eof | 5,294 | 10,231 | 0.5174× | 0.52× | ✅ |
| udp | 7,570 | 7,986 | 0.9479× | 0.95× | ✅ |
| connect | 17,898 | 18,707 | 0.9568× | 0.98× | ✅ |
| https | 108,733 | 108,803 | 0.9994× | 1.0× | ✅ |
| wss | 104,292 | 104,800 | 0.9951× | 1.0× | ✅ |
| atomic | 49,740 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,523 | 16,959 | 0.9153× | 0.94× | ✅ |
| http2-concurrent | 357,353 | 408,877 | 0.8740× | 0.88× | ✅ |
| websocket-64k | 156,004 | 159,699 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,786 | 16,331 | 0.7829× | 0.79× | ✅ |
| upload | 477,898 | 1,322,378 | 0.3614× | 0.37× | ✅ |
| static | 28,568 | 41,045 | 0.6960× | 0.72× | ✅ |
| pgsql | 23,221 | 24,595 | 0.9441× | 0.95× | ✅ |
| sqlite | 23,485 | 25,236 | 0.9306× | 0.94× | ✅ |
| curl | 85,724 | 90,483 | 0.9474× | 0.94× | ✅ |
| dns | 231,084 | 233,367 | 0.9902× | 0.99× | ✅ |

Geometric mean 0.8786×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
