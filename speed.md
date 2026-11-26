# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,210 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,051 | 23,170 | 0.9517× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,740 | 8,192 | 0.8227× | 0.81× | ✅ |
| timer | 202,185 | 313,879 | 0.6442× | 0.65× | ✅ |
| app | 127,538 | 135,928 | 0.9383× | 0.93× | ✅ |
| http2 | 19,172 | 20,497 | 0.9353× | 0.92× | ✅ |
| table | 194,281 | 201,882 | 0.9624× | 0.96× | ✅ |
| channel | 137,023 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,069 | 33,272 | 0.9638× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,653 | 35,793 | 0.9402× | 0.93× | ✅ |
| gzip | 194,288 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,984 | 181,570 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,494 | 18,730 | 0.9340× | 0.93× | ✅ |
| https | 106,288 | 108,806 | 0.9769× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,272 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,407 | 17,021 | 0.9052× | 0.89× | ✅ |
| http2-concurrent | 367,940 | 408,861 | 0.8999× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,692 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,897 | 1,322,430 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,598 | 19,721 | 0.9431× | 0.93× | ✅ |
| socket | 19,835 | 21,036 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,886 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,078 | 90,422 | 0.9520× | 0.95× | ✅ |
| dns | 230,167 | 232,625 | 0.9894× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
