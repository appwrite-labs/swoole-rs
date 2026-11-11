# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7c749ec` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,979 | 15,213 | 0.9188× | 0.93× | ✅ |
| echo-1k | 17,092 | 18,165 | 0.9409× | 0.95× | ✅ |
| json | 22,062 | 23,161 | 0.9525× | 0.96× | ✅ |
| service-health | 16,559 | 17,777 | 0.9315× | 0.94× | ✅ |
| websocket | 6,669 | 8,188 | 0.8146× | 0.82× | ✅ |
| timer | 202,058 | 313,551 | 0.6444× | 0.65× | ✅ |
| app | 125,662 | 135,913 | 0.9246× | 0.93× | ✅ |
| http2 | 19,749 | 20,468 | 0.9649× | 0.98× | ✅ |
| table | 195,767 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,065 | 154,654 | 0.8927× | 0.9× | ✅ |
| task | 31,981 | 33,265 | 0.9614× | 0.97× | ✅ |
| mysql | 49,482 | 51,061 | 0.9691× | 0.98× | ✅ |
| http-client | 33,425 | 35,809 | 0.9334× | 0.95× | ✅ |
| client | 18,452 | 19,757 | 0.9340× | 0.95× | ✅ |
| socket | 19,419 | 21,067 | 0.9218× | 0.94× | ✅ |
| gzip | 195,949 | 197,083 | 0.9942× | 1.0× | ✅ |
| websocket-deflate | 181,428 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,983 | 4,762 | 1.0463× | 1.09× | ➖ |
| tcp-eof | 5,525 | 10,231 | 0.5400× | 0.56× | ✅ |
| udp | 7,995 | 7,987 | 1.0010× | 1.0× | ❌ |
| connect | 18,275 | 18,707 | 0.9769× | 0.99× | ✅ |
| https | 108,875 | 108,810 | 1.0006× | 1.01× | ➖ |
| wss | 104,615 | 104,794 | 0.9983× | 1.0× | ✅ |
| atomic | 49,718 | 51,316 | 0.9689× | 0.98× | ✅ |
| process-mode | 16,462 | 16,959 | 0.9707× | 0.98× | ✅ |
| http2-concurrent | 371,884 | 408,880 | 0.9095× | 0.92× | ✅ |
| websocket-64k | 156,117 | 159,692 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 12,821 | 16,327 | 0.7853× | 0.79× | ✅ |
| upload | 544,189 | 1,322,382 | 0.4115× | 0.42× | ✅ |
| static | 34,369 | 40,981 | 0.8386× | 0.84× | ✅ |
| pgsql | 23,232 | 24,594 | 0.9446× | 0.96× | ✅ |
| sqlite | 23,525 | 25,232 | 0.9324× | 0.95× | ✅ |
| curl | 85,210 | 90,363 | 0.9430× | 0.95× | ✅ |
| dns | 231,388 | 232,905 | 0.9935× | 1.0× | ✅ |

Geometric mean 0.8960×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
