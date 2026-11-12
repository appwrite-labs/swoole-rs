# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2a4ee8b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,934 | 15,213 | 0.9159× | 0.92× | ✅ |
| echo-1k | 17,046 | 18,165 | 0.9384× | 0.94× | ✅ |
| json | 22,049 | 23,158 | 0.9521× | 0.95× | ✅ |
| service-health | 16,544 | 17,777 | 0.9306× | 0.93× | ✅ |
| websocket | 6,631 | 8,184 | 0.8102× | 0.82× | ✅ |
| timer | 203,308 | 313,714 | 0.6481× | 0.65× | ✅ |
| app | 125,827 | 136,006 | 0.9252× | 0.93× | ✅ |
| http2 | 18,644 | 20,468 | 0.9109× | 0.93× | ✅ |
| table | 195,754 | 201,865 | 0.9697× | 0.97× | ✅ |
| channel | 138,903 | 154,654 | 0.8981× | 0.9× | ✅ |
| task | 32,305 | 33,268 | 0.9710× | 0.96× | ✅ |
| mysql | 49,603 | 51,061 | 0.9714× | 0.97× | ✅ |
| http-client | 33,699 | 35,806 | 0.9412× | 0.94× | ✅ |
| client | 18,500 | 19,757 | 0.9364× | 0.94× | ✅ |
| socket | 19,623 | 21,067 | 0.9314× | 0.92× | ✅ |
| gzip | 196,017 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,391 | 181,568 | 0.9990× | 1.0× | ✅ |
| tcp-length | 4,839 | 4,762 | 1.0161× | 1.05× | ➖ |
| tcp-eof | 5,387 | 10,231 | 0.5265× | 0.54× | ✅ |
| udp | 7,619 | 7,987 | 0.9539× | 0.95× | ✅ |
| connect | 18,420 | 18,707 | 0.9847× | 0.98× | ✅ |
| https | 108,783 | 108,803 | 0.9998× | 1.0× | ✅ |
| wss | 104,529 | 104,797 | 0.9974× | 1.0× | ✅ |
| atomic | 49,705 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 16,781 | 16,959 | 0.9895× | 0.99× | ✅ |
| http2-concurrent | 359,355 | 408,880 | 0.8789× | 0.88× | ✅ |
| websocket-64k | 156,069 | 159,692 | 0.9773× | 0.98× | ✅ |
| websocket-fragmented | 12,863 | 16,331 | 0.7876× | 0.79× | ✅ |
| upload | 478,464 | 1,322,380 | 0.3618× | 0.37× | ✅ |
| static | 29,177 | 40,981 | 0.7120× | 0.72× | ✅ |
| pgsql | 23,201 | 24,595 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,484 | 25,221 | 0.9311× | 0.94× | ✅ |
| curl | 84,714 | 90,482 | 0.9363× | 0.94× | ✅ |
| dns | 230,395 | 232,594 | 0.9905× | 0.99× | ✅ |

Geometric mean 0.8845×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
