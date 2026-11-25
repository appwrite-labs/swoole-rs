# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,210 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,094 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,351 | 17,782 | 0.9195× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,016 | 313,788 | 0.6438× | 0.65× | ✅ |
| app | 127,493 | 135,929 | 0.9379× | 0.93× | ✅ |
| http2 | 19,036 | 20,496 | 0.9287× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,376 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,090 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,296 | 51,085 | 0.9846× | 0.97× | ✅ |
| http-client | 33,596 | 35,795 | 0.9386× | 0.93× | ✅ |
| gzip | 194,300 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,813 | 0.9769× | 0.98× | ✅ |
| wss | 101,579 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,431 | 17,021 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,356 | 409,053 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,507 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,856 | 41,005 | 0.6305× | 0.64× | ✅ |
| client | 18,609 | 19,730 | 0.9432× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,823 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,134 | 90,539 | 0.9514× | 0.95× | ✅ |
| dns | 230,140 | 232,573 | 0.9895× | 0.99× | ✅ |
| file | 108,940 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
