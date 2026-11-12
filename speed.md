# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9b16bbf` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,000 | 15,213 | 0.9202× | 0.92× | ✅ |
| echo-1k | 17,122 | 18,165 | 0.9425× | 0.94× | ✅ |
| json | 22,109 | 23,158 | 0.9547× | 0.95× | ✅ |
| service-health | 16,611 | 17,773 | 0.9346× | 0.93× | ✅ |
| websocket | 6,730 | 8,191 | 0.8216× | 0.82× | ✅ |
| timer | 203,004 | 313,562 | 0.6474× | 0.65× | ✅ |
| app | 125,933 | 135,956 | 0.9263× | 0.93× | ✅ |
| http2 | 18,959 | 20,463 | 0.9265× | 0.94× | ✅ |
| table | 195,807 | 201,865 | 0.9700× | 0.97× | ✅ |
| channel | 138,436 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,028 | 33,265 | 0.9628× | 0.96× | ✅ |
| mysql | 49,644 | 51,061 | 0.9722× | 0.97× | ✅ |
| http-client | 33,675 | 35,809 | 0.9404× | 0.94× | ✅ |
| client | 18,558 | 19,756 | 0.9393× | 0.94× | ✅ |
| socket | 19,673 | 21,067 | 0.9338× | 0.92× | ✅ |
| gzip | 196,080 | 197,079 | 0.9949× | 1.0× | ✅ |
| websocket-deflate | 181,490 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 4,995 | 4,762 | 1.0489× | 1.05× | ➖ |
| tcp-eof | 5,533 | 10,231 | 0.5408× | 0.54× | ✅ |
| udp | 7,544 | 7,987 | 0.9446× | 0.95× | ✅ |
| connect | 18,335 | 18,703 | 0.9803× | 0.98× | ✅ |
| https | 108,854 | 108,810 | 1.0004× | 1.0× | ❌ |
| wss | 104,616 | 104,794 | 0.9983× | 1.0× | ✅ |
| atomic | 49,758 | 51,316 | 0.9696× | 0.97× | ✅ |
| process-mode | 16,555 | 16,960 | 0.9761× | 0.97× | ✅ |
| http2-concurrent | 360,085 | 408,886 | 0.8806× | 0.89× | ✅ |
| websocket-64k | 156,186 | 159,698 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,116 | 16,331 | 0.8031× | 0.79× | ✅ |
| upload | 478,514 | 1,322,384 | 0.3619× | 0.42× | ✅ |
| static | 29,313 | 40,987 | 0.7152× | 0.84× | ✅ |
| pgsql | 23,312 | 24,594 | 0.9478× | 0.95× | ✅ |
| sqlite | 23,577 | 25,232 | 0.9344× | 0.94× | ✅ |
| curl | 85,087 | 90,406 | 0.9412× | 0.95× | ✅ |
| dns | 230,199 | 232,675 | 0.9894× | 0.99× | ✅ |

Geometric mean 0.8875×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
