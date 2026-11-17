# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c4a17d5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,820 | 15,210 | 0.9087× | 0.91× | ✅ |
| echo-1k | 16,828 | 18,165 | 0.9264× | 0.93× | ✅ |
| json | 21,980 | 23,161 | 0.9490× | 0.95× | ✅ |
| service-health | 16,385 | 17,773 | 0.9219× | 0.92× | ✅ |
| websocket | 6,616 | 8,191 | 0.8077× | 0.8× | ✅ |
| timer | 205,476 | 313,588 | 0.6552× | 0.65× | ✅ |
| app | 126,799 | 135,852 | 0.9334× | 0.93× | ✅ |
| http2 | 18,974 | 20,464 | 0.9272× | 0.92× | ✅ |
| table | 194,415 | 201,865 | 0.9631× | 0.97× | ✅ |
| channel | 137,348 | 155,174 | 0.8851× | 0.88× | ✅ |
| task | 32,477 | 33,264 | 0.9763× | 0.96× | ✅ |
| mysql | 49,919 | 51,058 | 0.9777× | 0.97× | ✅ |
| http-client | 33,680 | 35,804 | 0.9407× | 0.93× | ✅ |
| gzip | 194,491 | 197,083 | 0.9868× | 0.99× | ✅ |
| websocket-deflate | 179,018 | 181,572 | 0.9859× | 0.99× | ✅ |
| tcp-length | 4,583 | 4,762 | 0.9624× | 0.96× | ✅ |
| tcp-eof | 5,163 | 10,231 | 0.5046× | 0.5× | ✅ |
| udp | 7,488 | 7,986 | 0.9376× | 0.93× | ✅ |
| connect | 17,510 | 18,703 | 0.9362× | 0.93× | ✅ |
| https | 105,994 | 108,810 | 0.9741× | 0.98× | ✅ |
| wss | 101,045 | 104,797 | 0.9642× | 0.97× | ✅ |
| atomic | 49,910 | 51,316 | 0.9726× | 0.97× | ✅ |
| process-mode | 15,073 | 17,027 | 0.8852× | 0.88× | ✅ |
| http2-concurrent | 363,070 | 409,072 | 0.8875× | 0.88× | ✅ |
| websocket-64k | 89,864 | 159,696 | 0.5627× | 0.57× | ✅ |
| websocket-fragmented | 11,710 | 16,327 | 0.7172× | 0.72× | ✅ |
| upload | 415,964 | 1,322,387 | 0.3146× | 0.32× | ✅ |
| static | 26,702 | 41,051 | 0.6505× | 0.65× | ✅ |
| client | 18,557 | 19,756 | 0.9393× | 0.94× | ✅ |
| socket | 19,738 | 21,072 | 0.9367× | 0.92× | ✅ |
| pgsql | 22,796 | 24,594 | 0.9269× | 0.93× | ✅ |
| sqlite | 23,498 | 25,221 | 0.9317× | 0.93× | ✅ |
| curl | 86,475 | 90,353 | 0.9571× | 0.94× | ✅ |
| dns | 230,319 | 232,961 | 0.9887× | 0.99× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
