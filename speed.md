# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7251628` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,162 | 0.9258× | 0.92× | ✅ |
| json | 22,104 | 23,170 | 0.9540× | 0.95× | ✅ |
| service-health | 16,362 | 17,782 | 0.9201× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,957 | 313,818 | 0.6435× | 0.65× | ✅ |
| app | 127,629 | 135,966 | 0.9387× | 0.93× | ✅ |
| http2 | 18,957 | 20,499 | 0.9248× | 0.92× | ✅ |
| table | 194,327 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,727 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,644 | 35,796 | 0.9399× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,534 | 18,734 | 0.9360× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,214 | 51,285 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,438 | 17,025 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 363,420 | 409,053 | 0.8884× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,517 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,873 | 40,999 | 0.6311× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,881 | 21,045 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,829 | 24,663 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,139 | 90,420 | 0.9526× | 0.95× | ✅ |
| dns | 230,286 | 232,785 | 0.9893× | 0.99× | ✅ |
| file | 109,106 | 131,211 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
