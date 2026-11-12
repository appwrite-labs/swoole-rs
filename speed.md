# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5d66b9f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,999 | 15,214 | 0.9201× | 0.92× | ✅ |
| echo-1k | 17,120 | 18,165 | 0.9424× | 0.94× | ✅ |
| json | 22,103 | 23,162 | 0.9543× | 0.95× | ✅ |
| service-health | 16,602 | 17,777 | 0.9339× | 0.93× | ✅ |
| websocket | 6,729 | 8,184 | 0.8222× | 0.82× | ✅ |
| timer | 203,357 | 314,621 | 0.6464× | 0.65× | ✅ |
| app | 125,967 | 135,919 | 0.9268× | 0.93× | ✅ |
| http2 | 19,307 | 20,468 | 0.9433× | 0.94× | ✅ |
| table | 195,808 | 201,869 | 0.9700× | 0.97× | ✅ |
| channel | 138,437 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,014 | 33,265 | 0.9624× | 0.96× | ✅ |
| mysql | 49,645 | 51,050 | 0.9725× | 0.97× | ✅ |
| http-client | 33,670 | 35,805 | 0.9404× | 0.94× | ✅ |
| client | 18,545 | 19,757 | 0.9387× | 0.94× | ✅ |
| socket | 19,664 | 21,067 | 0.9334× | 0.92× | ✅ |
| gzip | 196,071 | 197,079 | 0.9949× | 1.0× | ✅ |
| websocket-deflate | 181,487 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 4,998 | 4,765 | 1.0488× | 1.05× | ➖ |
| tcp-eof | 5,562 | 10,228 | 0.5438× | 0.54× | ✅ |
| udp | 7,545 | 7,987 | 0.9447× | 0.95× | ✅ |
| connect | 18,317 | 18,703 | 0.9794× | 0.98× | ✅ |
| https | 108,855 | 108,810 | 1.0004× | 1.0× | ❌ |
| wss | 104,613 | 104,800 | 0.9982× | 1.0× | ✅ |
| atomic | 49,756 | 51,316 | 0.9696× | 0.97× | ✅ |
| process-mode | 16,562 | 17,021 | 0.9730× | 0.97× | ✅ |
| http2-concurrent | 361,795 | 408,880 | 0.8848× | 0.89× | ✅ |
| websocket-64k | 156,172 | 159,692 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,093 | 16,331 | 0.8017× | 0.79× | ✅ |
| upload | 478,502 | 1,322,382 | 0.3618× | 0.42× | ✅ |
| static | 29,309 | 41,016 | 0.7146× | 0.84× | ✅ |
| pgsql | 23,284 | 24,595 | 0.9467× | 0.95× | ✅ |
| sqlite | 23,551 | 25,232 | 0.9334× | 0.94× | ✅ |
| curl | 85,064 | 90,511 | 0.9398× | 0.95× | ✅ |
| dns | 230,878 | 233,191 | 0.9901× | 0.99× | ✅ |

Geometric mean 0.8879×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
