# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6fa833a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,744 | 15,210 | 0.9036× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,162 | 0.9228× | 0.92× | ✅ |
| json | 22,076 | 23,170 | 0.9528× | 0.95× | ✅ |
| service-health | 16,334 | 17,782 | 0.9186× | 0.92× | ✅ |
| websocket | 6,671 | 8,192 | 0.8143× | 0.81× | ✅ |
| timer | 201,535 | 313,861 | 0.6421× | 0.65× | ✅ |
| app | 127,020 | 135,988 | 0.9341× | 0.93× | ✅ |
| http2 | 18,960 | 20,501 | 0.9249× | 0.92× | ✅ |
| table | 194,298 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,956 | 33,264 | 0.9607× | 0.96× | ✅ |
| mysql | 50,050 | 50,997 | 0.9814× | 0.97× | ✅ |
| http-client | 33,363 | 35,797 | 0.9320× | 0.93× | ✅ |
| gzip | 194,366 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,099 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,228 | 0.5080× | 0.5× | ✅ |
| udp | 7,560 | 7,987 | 0.9466× | 0.95× | ✅ |
| connect | 17,577 | 18,734 | 0.9382× | 0.93× | ✅ |
| https | 106,004 | 108,802 | 0.9743× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,184 | 51,276 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,267 | 17,021 | 0.8970× | 0.89× | ✅ |
| http2-concurrent | 364,592 | 408,861 | 0.8917× | 0.89× | ✅ |
| websocket-64k | 89,970 | 159,692 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,368 | 0.7233× | 0.72× | ✅ |
| upload | 416,433 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,109 | 41,063 | 0.6602× | 0.65× | ✅ |
| client | 18,478 | 19,717 | 0.9372× | 0.93× | ✅ |
| socket | 19,705 | 21,036 | 0.9367× | 0.93× | ✅ |
| pgsql | 23,090 | 24,662 | 0.9363× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,922 | 90,536 | 0.9490× | 0.95× | ✅ |
| dns | 229,729 | 232,932 | 0.9862× | 0.99× | ✅ |
| file | 108,210 | 131,210 | 0.8247× | 0.83× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
