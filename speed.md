# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f492aa9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,881 | 15,210 | 0.9126× | 0.91× | ✅ |
| echo-1k | 16,916 | 18,162 | 0.9314× | 0.93× | ✅ |
| json | 22,040 | 23,161 | 0.9516× | 0.95× | ✅ |
| service-health | 16,474 | 17,773 | 0.9269× | 0.93× | ✅ |
| websocket | 6,509 | 8,191 | 0.7946× | 0.8× | ✅ |
| timer | 206,847 | 313,506 | 0.6598× | 0.65× | ✅ |
| app | 126,013 | 135,934 | 0.9270× | 0.93× | ✅ |
| http2 | 18,831 | 20,464 | 0.9202× | 0.92× | ✅ |
| table | 194,486 | 201,865 | 0.9634× | 0.97× | ✅ |
| channel | 137,418 | 155,171 | 0.8856× | 0.88× | ✅ |
| task | 32,544 | 33,265 | 0.9783× | 0.96× | ✅ |
| mysql | 49,651 | 51,061 | 0.9724× | 0.97× | ✅ |
| http-client | 33,519 | 35,800 | 0.9363× | 0.93× | ✅ |
| gzip | 193,984 | 197,079 | 0.9843× | 0.99× | ✅ |
| websocket-deflate | 178,709 | 181,572 | 0.9842× | 0.99× | ✅ |
| tcp-length | 4,522 | 4,762 | 0.9495× | 0.96× | ✅ |
| tcp-eof | 5,124 | 10,231 | 0.5008× | 0.5× | ✅ |
| udp | 7,421 | 7,986 | 0.9292× | 0.93× | ✅ |
| connect | 17,427 | 18,707 | 0.9316× | 0.93× | ✅ |
| https | 105,935 | 108,806 | 0.9736× | 0.98× | ✅ |
| wss | 100,775 | 104,794 | 0.9616× | 0.97× | ✅ |
| atomic | 49,974 | 51,312 | 0.9739× | 0.97× | ✅ |
| process-mode | 15,061 | 16,956 | 0.8882× | 0.88× | ✅ |
| http2-concurrent | 360,638 | 409,429 | 0.8808× | 0.88× | ✅ |
| websocket-64k | 89,758 | 159,696 | 0.5621× | 0.57× | ✅ |
| websocket-fragmented | 11,663 | 16,327 | 0.7143× | 0.72× | ✅ |
| upload | 414,003 | 1,322,387 | 0.3131× | 0.32× | ✅ |
| static | 26,943 | 41,016 | 0.6569× | 0.65× | ✅ |
| client | 18,546 | 19,757 | 0.9387× | 0.94× | ✅ |
| socket | 19,703 | 21,073 | 0.9350× | 0.92× | ✅ |
| pgsql | 23,046 | 24,595 | 0.9370× | 0.93× | ✅ |
| sqlite | 23,533 | 25,236 | 0.9325× | 0.93× | ✅ |
| curl | 84,902 | 90,483 | 0.9383× | 0.94× | ✅ |
| dns | 229,215 | 232,942 | 0.9840× | 0.99× | ✅ |

Geometric mean 0.8516×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
