# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ac0c4c1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,947 | 15,214 | 0.9168× | 0.92× | ✅ |
| echo-1k | 17,060 | 18,165 | 0.9391× | 0.94× | ✅ |
| json | 22,149 | 23,161 | 0.9563× | 0.95× | ✅ |
| service-health | 16,585 | 17,777 | 0.9330× | 0.93× | ✅ |
| websocket | 6,564 | 8,188 | 0.8017× | 0.8× | ✅ |
| timer | 207,353 | 313,504 | 0.6614× | 0.65× | ✅ |
| app | 125,653 | 135,918 | 0.9245× | 0.93× | ✅ |
| http2 | 18,653 | 20,468 | 0.9113× | 0.91× | ✅ |
| table | 195,590 | 201,865 | 0.9689× | 0.97× | ✅ |
| channel | 138,061 | 155,174 | 0.8897× | 0.89× | ✅ |
| task | 32,483 | 33,268 | 0.9764× | 0.96× | ✅ |
| mysql | 49,698 | 51,062 | 0.9733× | 0.97× | ✅ |
| http-client | 33,968 | 35,800 | 0.9489× | 0.94× | ✅ |
| client | 18,624 | 19,756 | 0.9427× | 0.94× | ✅ |
| socket | 19,751 | 21,072 | 0.9373× | 0.92× | ✅ |
| gzip | 194,175 | 197,083 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,050 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,729 | 4,765 | 0.9924× | 0.99× | ✅ |
| tcp-eof | 5,222 | 10,231 | 0.5104× | 0.51× | ✅ |
| udp | 7,504 | 7,986 | 0.9396× | 0.93× | ✅ |
| connect | 17,705 | 18,703 | 0.9466× | 0.94× | ✅ |
| https | 108,218 | 108,810 | 0.9946× | 1.0× | ✅ |
| wss | 103,808 | 104,797 | 0.9906× | 0.99× | ✅ |
| atomic | 49,734 | 51,316 | 0.9692× | 0.97× | ✅ |
| process-mode | 15,081 | 17,027 | 0.8857× | 0.88× | ✅ |
| http2-concurrent | 359,728 | 409,069 | 0.8794× | 0.88× | ✅ |
| websocket-64k | 89,906 | 159,694 | 0.5630× | 0.57× | ✅ |
| websocket-fragmented | 11,819 | 16,327 | 0.7239× | 0.74× | ✅ |
| upload | 412,417 | 1,322,387 | 0.3119× | 0.32× | ✅ |
| static | 27,143 | 41,080 | 0.6607× | 0.66× | ✅ |
| pgsql | 23,364 | 24,595 | 0.9499× | 0.95× | ✅ |
| sqlite | 23,566 | 25,236 | 0.9338× | 0.93× | ✅ |
| curl | 86,130 | 90,479 | 0.9519× | 0.94× | ✅ |
| dns | 231,010 | 232,879 | 0.9920× | 0.99× | ✅ |

Geometric mean 0.8576×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
