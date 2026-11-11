# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d0e2006` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,923 | 15,213 | 0.9152× | 0.93× | ✅ |
| echo-1k | 17,040 | 18,166 | 0.9380× | 0.95× | ✅ |
| json | 22,012 | 23,161 | 0.9504× | 0.96× | ✅ |
| service-health | 16,506 | 17,777 | 0.9285× | 0.94× | ✅ |
| websocket | 6,669 | 8,191 | 0.8142× | 0.82× | ✅ |
| timer | 202,016 | 313,643 | 0.6441× | 0.65× | ✅ |
| app | 125,560 | 135,931 | 0.9237× | 0.93× | ✅ |
| http2 | 19,703 | 20,467 | 0.9627× | 0.98× | ✅ |
| table | 195,779 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,012 | 154,655 | 0.8924× | 0.9× | ✅ |
| task | 31,933 | 33,260 | 0.9601× | 0.97× | ✅ |
| mysql | 49,418 | 51,061 | 0.9678× | 0.98× | ✅ |
| http-client | 33,370 | 35,809 | 0.9319× | 0.95× | ✅ |
| client | 18,393 | 19,757 | 0.9310× | 0.95× | ✅ |
| socket | 19,363 | 21,067 | 0.9191× | 0.94× | ✅ |
| gzip | 195,884 | 197,083 | 0.9939× | 1.0× | ✅ |
| websocket-deflate | 181,431 | 181,572 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,983 | 4,762 | 1.0463× | 1.09× | ➖ |
| tcp-eof | 5,525 | 10,228 | 0.5401× | 0.56× | ✅ |
| udp | 7,994 | 7,987 | 1.0010× | 1.0× | ❌ |
| connect | 18,219 | 18,703 | 0.9741× | 0.99× | ✅ |
| https | 108,837 | 108,806 | 1.0003× | 1.01× | ➖ |
| wss | 104,611 | 104,801 | 0.9982× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9678× | 0.98× | ✅ |
| process-mode | 16,417 | 16,956 | 0.9682× | 0.98× | ✅ |
| http2-concurrent | 371,995 | 408,882 | 0.9098× | 0.92× | ✅ |
| websocket-64k | 156,116 | 159,695 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 13,024 | 16,331 | 0.7975× | 0.79× | ✅ |
| upload | 544,654 | 1,322,381 | 0.4119× | 0.42× | ✅ |
| static | 34,355 | 40,981 | 0.8383× | 0.84× | ✅ |
| pgsql | 23,177 | 24,595 | 0.9424× | 0.96× | ✅ |
| sqlite | 23,469 | 25,232 | 0.9301× | 0.95× | ✅ |
| curl | 85,159 | 90,384 | 0.9422× | 0.95× | ✅ |
| dns | 231,146 | 233,170 | 0.9913× | 1.0× | ✅ |

Geometric mean 0.8953×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
