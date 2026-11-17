# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be7bd8d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,743 | 15,214 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,720 | 18,162 | 0.9206× | 0.93× | ✅ |
| json | 22,007 | 23,158 | 0.9503× | 0.95× | ✅ |
| service-health | 16,319 | 17,773 | 0.9182× | 0.92× | ✅ |
| websocket | 6,620 | 8,191 | 0.8081× | 0.8× | ✅ |
| timer | 203,040 | 313,869 | 0.6469× | 0.65× | ✅ |
| app | 126,251 | 135,946 | 0.9287× | 0.93× | ✅ |
| http2 | 18,964 | 20,495 | 0.9253× | 0.92× | ✅ |
| table | 193,874 | 201,876 | 0.9604× | 0.96× | ✅ |
| channel | 137,142 | 155,717 | 0.8807× | 0.88× | ✅ |
| task | 31,861 | 33,264 | 0.9578× | 0.96× | ✅ |
| mysql | 49,909 | 51,061 | 0.9774× | 0.97× | ✅ |
| http-client | 33,533 | 35,804 | 0.9366× | 0.93× | ✅ |
| gzip | 194,194 | 197,097 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,100 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,575 | 4,765 | 0.9600× | 0.96× | ✅ |
| tcp-eof | 5,086 | 10,228 | 0.4972× | 0.5× | ✅ |
| udp | 7,555 | 7,986 | 0.9460× | 0.93× | ✅ |
| connect | 17,339 | 18,686 | 0.9279× | 0.93× | ✅ |
| https | 105,946 | 108,813 | 0.9737× | 0.98× | ✅ |
| wss | 101,068 | 104,797 | 0.9644× | 0.97× | ✅ |
| atomic | 49,817 | 51,276 | 0.9716× | 0.97× | ✅ |
| process-mode | 15,253 | 16,959 | 0.8994× | 0.89× | ✅ |
| http2-concurrent | 364,305 | 409,069 | 0.8906× | 0.88× | ✅ |
| websocket-64k | 89,900 | 159,690 | 0.5630× | 0.57× | ✅ |
| websocket-fragmented | 11,750 | 16,365 | 0.7180× | 0.72× | ✅ |
| upload | 415,944 | 1,322,413 | 0.3145× | 0.32× | ✅ |
| static | 26,576 | 40,960 | 0.6488× | 0.65× | ✅ |
| client | 18,488 | 19,756 | 0.9358× | 0.94× | ✅ |
| socket | 19,649 | 21,068 | 0.9326× | 0.94× | ✅ |
| pgsql | 22,702 | 24,595 | 0.9231× | 0.93× | ✅ |
| sqlite | 23,438 | 25,236 | 0.9287× | 0.93× | ✅ |
| curl | 86,312 | 90,350 | 0.9553× | 0.94× | ✅ |
| dns | 230,034 | 232,679 | 0.9886× | 0.99× | ✅ |

Geometric mean 0.8513×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
