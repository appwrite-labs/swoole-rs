# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `943668b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,934 | 15,210 | 0.9161× | 0.92× | ✅ |
| echo-1k | 17,046 | 18,162 | 0.9386× | 0.94× | ✅ |
| json | 22,040 | 23,161 | 0.9516× | 0.95× | ✅ |
| service-health | 16,535 | 17,777 | 0.9301× | 0.93× | ✅ |
| websocket | 6,631 | 8,188 | 0.8099× | 0.82× | ✅ |
| timer | 203,250 | 313,515 | 0.6483× | 0.65× | ✅ |
| app | 125,887 | 135,871 | 0.9265× | 0.93× | ✅ |
| http2 | 18,637 | 20,465 | 0.9107× | 0.93× | ✅ |
| table | 195,745 | 201,865 | 0.9697× | 0.97× | ✅ |
| channel | 138,893 | 154,651 | 0.8981× | 0.9× | ✅ |
| task | 32,313 | 33,265 | 0.9714× | 0.96× | ✅ |
| mysql | 49,602 | 51,044 | 0.9717× | 0.97× | ✅ |
| http-client | 33,688 | 35,809 | 0.9408× | 0.94× | ✅ |
| client | 18,491 | 19,757 | 0.9359× | 0.94× | ✅ |
| socket | 19,614 | 21,067 | 0.9310× | 0.92× | ✅ |
| gzip | 196,017 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,395 | 181,572 | 0.9990× | 1.0× | ✅ |
| tcp-length | 4,839 | 4,762 | 1.0161× | 1.05× | ➖ |
| tcp-eof | 5,387 | 10,231 | 0.5265× | 0.54× | ✅ |
| udp | 7,619 | 7,986 | 0.9540× | 0.95× | ✅ |
| connect | 18,421 | 18,707 | 0.9847× | 0.98× | ✅ |
| https | 108,788 | 108,813 | 0.9998× | 1.0× | ✅ |
| wss | 104,518 | 104,800 | 0.9973× | 1.0× | ✅ |
| atomic | 49,696 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 16,775 | 16,959 | 0.9891× | 0.97× | ✅ |
| http2-concurrent | 359,179 | 408,882 | 0.8784× | 0.88× | ✅ |
| websocket-64k | 156,078 | 159,696 | 0.9773× | 0.98× | ✅ |
| websocket-fragmented | 12,866 | 16,331 | 0.7878× | 0.79× | ✅ |
| upload | 478,461 | 1,322,380 | 0.3618× | 0.37× | ✅ |
| static | 29,237 | 41,039 | 0.7124× | 0.72× | ✅ |
| pgsql | 23,205 | 24,594 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,484 | 25,236 | 0.9306× | 0.94× | ✅ |
| curl | 84,711 | 90,476 | 0.9363× | 0.94× | ✅ |
| dns | 230,078 | 232,832 | 0.9882× | 0.99× | ✅ |

Geometric mean 0.8844×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
