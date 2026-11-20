# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6f75fb1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,101 | 23,174 | 0.9537× | 0.95× | ✅ |
| service-health | 16,359 | 17,786 | 0.9197× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 202,009 | 313,732 | 0.6439× | 0.65× | ✅ |
| app | 127,625 | 135,949 | 0.9388× | 0.93× | ✅ |
| http2 | 18,970 | 20,493 | 0.9257× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,383 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,084 | 33,265 | 0.9645× | 0.96× | ✅ |
| mysql | 50,325 | 51,082 | 0.9852× | 0.97× | ✅ |
| http-client | 33,646 | 35,796 | 0.9399× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,534 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,331 | 108,813 | 0.9772× | 0.98× | ✅ |
| wss | 101,591 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 363,612 | 409,056 | 0.8889× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,531 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,896 | 41,034 | 0.6311× | 0.64× | ✅ |
| client | 18,642 | 19,726 | 0.9450× | 0.93× | ✅ |
| socket | 19,878 | 21,046 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,829 | 24,663 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,140 | 90,419 | 0.9527× | 0.95× | ✅ |
| dns | 230,227 | 233,126 | 0.9876× | 0.99× | ✅ |
| file | 109,101 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
