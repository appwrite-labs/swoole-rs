# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `041cde2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,210 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,811 | 18,162 | 0.9256× | 0.92× | ✅ |
| json | 22,103 | 23,161 | 0.9543× | 0.95× | ✅ |
| service-health | 16,353 | 17,773 | 0.9201× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 202,024 | 313,966 | 0.6435× | 0.65× | ✅ |
| app | 127,612 | 135,918 | 0.9389× | 0.93× | ✅ |
| http2 | 18,963 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,386 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,378 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,090 | 33,265 | 0.9647× | 0.96× | ✅ |
| mysql | 50,320 | 51,086 | 0.9850× | 0.97× | ✅ |
| http-client | 33,664 | 35,789 | 0.9406× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,613 | 7,986 | 0.9533× | 0.95× | ✅ |
| connect | 17,530 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,299 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,573 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,414 | 17,024 | 0.9054× | 0.89× | ✅ |
| http2-concurrent | 363,669 | 409,057 | 0.8890× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,926 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,550 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,903 | 41,092 | 0.6304× | 0.64× | ✅ |
| client | 18,624 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,863 | 21,046 | 0.9438× | 0.93× | ✅ |
| pgsql | 22,823 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,124 | 90,418 | 0.9525× | 0.95× | ✅ |
| dns | 230,097 | 233,332 | 0.9861× | 0.99× | ✅ |
| file | 109,092 | 131,296 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
