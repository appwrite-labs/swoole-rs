# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e64134a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,210 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,812 | 18,162 | 0.9257× | 0.92× | ✅ |
| json | 22,094 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,352 | 17,782 | 0.9196× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 202,016 | 313,812 | 0.6437× | 0.65× | ✅ |
| app | 127,645 | 135,947 | 0.9389× | 0.93× | ✅ |
| http2 | 18,975 | 20,497 | 0.9258× | 0.92× | ✅ |
| table | 194,321 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,373 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,303 | 51,085 | 0.9847× | 0.97× | ✅ |
| http-client | 33,639 | 35,787 | 0.9400× | 0.93× | ✅ |
| gzip | 194,278 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,533 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,575 | 104,793 | 0.9693× | 0.97× | ✅ |
| atomic | 50,207 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,420 | 17,021 | 0.9059× | 0.89× | ✅ |
| http2-concurrent | 363,685 | 409,057 | 0.8891× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,508 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,919 | 41,071 | 0.6311× | 0.64× | ✅ |
| client | 18,629 | 19,730 | 0.9442× | 0.93× | ✅ |
| socket | 19,871 | 21,045 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,818 | 24,663 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,130 | 90,415 | 0.9526× | 0.95× | ✅ |
| dns | 230,385 | 232,870 | 0.9893× | 0.99× | ✅ |
| file | 109,074 | 131,296 | 0.8307× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
