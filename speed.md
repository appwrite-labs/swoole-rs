# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `71edaa0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,918 | 15,210 | 0.9150× | 0.92× | ✅ |
| echo-1k | 17,018 | 18,166 | 0.9368× | 0.94× | ✅ |
| json | 22,019 | 23,162 | 0.9507× | 0.95× | ✅ |
| service-health | 16,518 | 17,773 | 0.9294× | 0.93× | ✅ |
| websocket | 6,523 | 8,191 | 0.7964× | 0.81× | ✅ |
| timer | 203,018 | 313,598 | 0.6474× | 0.65× | ✅ |
| app | 125,416 | 135,936 | 0.9226× | 0.93× | ✅ |
| http2 | 18,570 | 20,464 | 0.9074× | 0.92× | ✅ |
| table | 195,492 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,433 | 154,654 | 0.8886× | 0.89× | ✅ |
| task | 31,957 | 33,262 | 0.9608× | 0.96× | ✅ |
| mysql | 49,532 | 51,064 | 0.9700× | 0.97× | ✅ |
| http-client | 33,680 | 35,805 | 0.9407× | 0.94× | ✅ |
| client | 18,469 | 19,757 | 0.9348× | 0.94× | ✅ |
| socket | 19,597 | 21,067 | 0.9302× | 0.92× | ✅ |
| gzip | 194,109 | 197,079 | 0.9849× | 0.99× | ✅ |
| websocket-deflate | 179,309 | 181,568 | 0.9876× | 0.99× | ✅ |
| tcp-length | 4,738 | 4,765 | 0.9942× | 1.01× | ✅ |
| tcp-eof | 5,261 | 10,228 | 0.5144× | 0.52× | ✅ |
| udp | 7,532 | 7,986 | 0.9432× | 0.95× | ✅ |
| connect | 17,855 | 18,707 | 0.9544× | 0.98× | ✅ |
| https | 108,184 | 108,803 | 0.9943× | 1.0× | ✅ |
| wss | 103,798 | 104,794 | 0.9905× | 1.0× | ✅ |
| atomic | 49,635 | 51,316 | 0.9672× | 0.97× | ✅ |
| process-mode | 15,096 | 16,960 | 0.8901× | 0.94× | ✅ |
| http2-concurrent | 356,789 | 408,882 | 0.8726× | 0.88× | ✅ |
| websocket-64k | 155,971 | 159,695 | 0.9767× | 0.98× | ✅ |
| websocket-fragmented | 12,756 | 16,331 | 0.7811× | 0.79× | ✅ |
| upload | 477,858 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 27,645 | 41,045 | 0.6735× | 0.72× | ✅ |
| pgsql | 23,197 | 24,594 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,468 | 25,236 | 0.9299× | 0.94× | ✅ |
| curl | 85,700 | 90,464 | 0.9473× | 0.94× | ✅ |
| dns | 230,753 | 233,024 | 0.9903× | 0.99× | ✅ |

Geometric mean 0.8754×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
