# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `44ce72a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,777 | 15,214 | 0.9056× | 0.91× | ✅ |
| echo-1k | 16,791 | 18,165 | 0.9243× | 0.93× | ✅ |
| json | 21,934 | 23,161 | 0.9470× | 0.95× | ✅ |
| service-health | 16,347 | 17,777 | 0.9195× | 0.92× | ✅ |
| websocket | 6,533 | 8,188 | 0.7980× | 0.8× | ✅ |
| timer | 205,830 | 313,531 | 0.6565× | 0.65× | ✅ |
| app | 126,743 | 135,949 | 0.9323× | 0.93× | ✅ |
| http2 | 19,105 | 20,468 | 0.9334× | 0.92× | ✅ |
| table | 194,802 | 201,865 | 0.9650× | 0.97× | ✅ |
| channel | 137,555 | 155,171 | 0.8865× | 0.88× | ✅ |
| task | 32,482 | 33,264 | 0.9765× | 0.96× | ✅ |
| mysql | 49,965 | 51,061 | 0.9785× | 0.97× | ✅ |
| http-client | 33,651 | 35,803 | 0.9399× | 0.93× | ✅ |
| gzip | 194,446 | 197,079 | 0.9866× | 0.99× | ✅ |
| websocket-deflate | 178,979 | 181,568 | 0.9857× | 0.99× | ✅ |
| tcp-length | 4,566 | 4,765 | 0.9581× | 0.96× | ✅ |
| tcp-eof | 5,146 | 10,231 | 0.5029× | 0.5× | ✅ |
| udp | 7,467 | 7,986 | 0.9350× | 0.93× | ✅ |
| connect | 17,456 | 18,707 | 0.9331× | 0.93× | ✅ |
| https | 105,936 | 108,813 | 0.9736× | 0.98× | ✅ |
| wss | 100,965 | 104,797 | 0.9634× | 0.97× | ✅ |
| atomic | 49,849 | 51,316 | 0.9714× | 0.97× | ✅ |
| process-mode | 14,998 | 16,963 | 0.8842× | 0.88× | ✅ |
| http2-concurrent | 365,982 | 409,069 | 0.8947× | 0.88× | ✅ |
| websocket-64k | 89,785 | 159,699 | 0.5622× | 0.57× | ✅ |
| websocket-fragmented | 11,600 | 16,327 | 0.7105× | 0.72× | ✅ |
| upload | 415,349 | 1,322,387 | 0.3141× | 0.32× | ✅ |
| static | 26,632 | 41,080 | 0.6483× | 0.65× | ✅ |
| client | 18,573 | 19,757 | 0.9401× | 0.94× | ✅ |
| socket | 19,714 | 21,072 | 0.9356× | 0.92× | ✅ |
| pgsql | 22,769 | 24,594 | 0.9258× | 0.93× | ✅ |
| sqlite | 23,427 | 25,236 | 0.9283× | 0.93× | ✅ |
| curl | 86,324 | 90,350 | 0.9554× | 0.94× | ✅ |
| dns | 230,709 | 233,399 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8523×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
