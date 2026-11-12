# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `88b2601` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,213 | 0.9154× | 0.92× | ✅ |
| echo-1k | 17,075 | 18,165 | 0.9400× | 0.94× | ✅ |
| json | 22,005 | 23,161 | 0.9501× | 0.96× | ✅ |
| service-health | 16,502 | 17,777 | 0.9283× | 0.93× | ✅ |
| websocket | 6,714 | 8,188 | 0.8201× | 0.82× | ✅ |
| timer | 202,514 | 313,953 | 0.6450× | 0.65× | ✅ |
| app | 125,890 | 135,941 | 0.9261× | 0.93× | ✅ |
| http2 | 19,831 | 20,464 | 0.9691× | 0.97× | ✅ |
| table | 195,719 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,803 | 154,654 | 0.8975× | 0.9× | ✅ |
| task | 31,968 | 33,265 | 0.9610× | 0.96× | ✅ |
| mysql | 49,537 | 51,041 | 0.9705× | 0.97× | ✅ |
| http-client | 33,586 | 35,806 | 0.9380× | 0.94× | ✅ |
| client | 18,461 | 19,758 | 0.9344× | 0.94× | ✅ |
| socket | 19,735 | 21,067 | 0.9368× | 0.92× | ✅ |
| gzip | 196,006 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,473 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,762 | 1.0545× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,231 | 0.5438× | 0.54× | ✅ |
| udp | 8,034 | 7,986 | 1.0059× | 1.0× | ❌ |
| connect | 18,254 | 18,703 | 0.9760× | 0.98× | ✅ |
| https | 108,817 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,667 | 104,800 | 0.9987× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,420 | 16,959 | 0.9682× | 0.97× | ✅ |
| http2-concurrent | 373,256 | 408,877 | 0.9129× | 0.92× | ✅ |
| websocket-64k | 156,167 | 159,695 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,069 | 16,331 | 0.8002× | 0.79× | ✅ |
| upload | 544,637 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,487 | 41,053 | 0.8401× | 0.84× | ✅ |
| pgsql | 23,211 | 24,594 | 0.9438× | 0.95× | ✅ |
| sqlite | 23,487 | 25,232 | 0.9309× | 0.94× | ✅ |
| curl | 85,231 | 90,337 | 0.9435× | 0.95× | ✅ |
| dns | 231,493 | 232,650 | 0.9950× | 1.0× | ✅ |

Geometric mean 0.8978×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
