# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `839a515` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,975 | 15,213 | 0.9186× | 0.92× | ✅ |
| echo-1k | 17,090 | 18,162 | 0.9410× | 0.94× | ✅ |
| json | 22,078 | 23,161 | 0.9532× | 0.95× | ✅ |
| service-health | 16,577 | 17,773 | 0.9327× | 0.93× | ✅ |
| websocket | 6,577 | 8,191 | 0.8029× | 0.81× | ✅ |
| timer | 203,707 | 313,526 | 0.6497× | 0.65× | ✅ |
| app | 125,331 | 135,937 | 0.9220× | 0.93× | ✅ |
| http2 | 18,672 | 20,464 | 0.9124× | 0.92× | ✅ |
| table | 195,526 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,467 | 154,651 | 0.8889× | 0.89× | ✅ |
| task | 32,023 | 33,268 | 0.9626× | 0.96× | ✅ |
| mysql | 49,509 | 51,064 | 0.9695× | 0.97× | ✅ |
| http-client | 33,691 | 35,805 | 0.9410× | 0.94× | ✅ |
| client | 18,484 | 19,757 | 0.9356× | 0.94× | ✅ |
| socket | 19,612 | 21,067 | 0.9309× | 0.92× | ✅ |
| gzip | 194,173 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,382 | 181,572 | 0.9879× | 0.99× | ✅ |
| tcp-length | 4,795 | 4,762 | 1.0069× | 1.01× | ➖ |
| tcp-eof | 5,308 | 10,231 | 0.5188× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,422 | 18,707 | 0.9848× | 0.98× | ✅ |
| https | 108,811 | 108,803 | 1.0001× | 1.0× | ❌ |
| wss | 104,383 | 104,801 | 0.9960× | 1.0× | ✅ |
| atomic | 49,740 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,870 | 16,959 | 0.9358× | 0.99× | ✅ |
| http2-concurrent | 359,652 | 409,522 | 0.8782× | 0.88× | ✅ |
| websocket-64k | 156,027 | 159,695 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,804 | 16,331 | 0.7840× | 0.79× | ✅ |
| upload | 478,096 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,185 | 41,045 | 0.7110× | 0.72× | ✅ |
| pgsql | 23,236 | 24,595 | 0.9448× | 0.95× | ✅ |
| sqlite | 23,504 | 25,236 | 0.9314× | 0.94× | ✅ |
| curl | 84,877 | 90,316 | 0.9398× | 0.94× | ✅ |
| dns | 230,666 | 232,808 | 0.9908× | 0.99× | ✅ |

Geometric mean 0.8811×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
