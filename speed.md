# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5d6591a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,166 | 0.9249× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,966 | 313,966 | 0.6433× | 0.65× | ✅ |
| app | 127,619 | 135,942 | 0.9388× | 0.93× | ✅ |
| http2 | 18,959 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,310 | 201,889 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,628 | 35,797 | 0.9394× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,764 | 0.9784× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,415 | 17,024 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,183 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,527 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 41,028 | 0.6300× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,818 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,133 | 90,422 | 0.9526× | 0.95× | ✅ |
| dns | 230,134 | 233,137 | 0.9871× | 0.99× | ✅ |
| file | 109,088 | 131,300 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
