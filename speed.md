# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,214 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,170 | 0.9535× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,082 | 313,801 | 0.6440× | 0.65× | ✅ |
| app | 127,528 | 135,949 | 0.9381× | 0.93× | ✅ |
| http2 | 19,038 | 20,496 | 0.9288× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,372 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,090 | 33,265 | 0.9647× | 0.96× | ✅ |
| mysql | 50,273 | 51,084 | 0.9841× | 0.97× | ✅ |
| http-client | 33,620 | 35,787 | 0.9395× | 0.93× | ✅ |
| gzip | 194,284 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 364,298 | 409,053 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,510 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,914 | 41,063 | 0.6311× | 0.64× | ✅ |
| client | 18,613 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,856 | 21,042 | 0.9436× | 0.93× | ✅ |
| pgsql | 22,819 | 24,663 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,132 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 229,866 | 232,656 | 0.9880× | 0.99× | ✅ |
| file | 108,940 | 131,207 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
