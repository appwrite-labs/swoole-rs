# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cb1159b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,214 | 0.9044× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,166 | 0.9248× | 0.92× | ✅ |
| json | 22,096 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,185 | 0.8212× | 0.81× | ✅ |
| timer | 202,099 | 313,941 | 0.6437× | 0.65× | ✅ |
| app | 127,529 | 135,860 | 0.9387× | 0.93× | ✅ |
| http2 | 18,985 | 20,500 | 0.9261× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,379 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,289 | 51,084 | 0.9844× | 0.97× | ✅ |
| http-client | 33,638 | 35,788 | 0.9399× | 0.93× | ✅ |
| gzip | 194,277 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,533 | 18,730 | 0.9361× | 0.93× | ✅ |
| https | 106,300 | 108,813 | 0.9769× | 0.98× | ✅ |
| wss | 101,581 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,209 | 51,276 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,436 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 364,221 | 408,861 | 0.8908× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,509 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,885 | 41,034 | 0.6308× | 0.64× | ✅ |
| client | 18,616 | 19,721 | 0.9440× | 0.93× | ✅ |
| socket | 19,859 | 21,036 | 0.9440× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,137 | 90,543 | 0.9513× | 0.95× | ✅ |
| dns | 229,870 | 233,142 | 0.9860× | 0.99× | ✅ |
| file | 108,959 | 131,300 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
