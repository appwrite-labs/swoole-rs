# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0472dbd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,104 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,782 | 0.9201× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 202,004 | 313,804 | 0.6437× | 0.65× | ✅ |
| app | 127,636 | 135,970 | 0.9387× | 0.93× | ✅ |
| http2 | 18,958 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,327 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,288 | 50,959 | 0.9868× | 0.97× | ✅ |
| http-client | 33,646 | 35,797 | 0.9399× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,209 | 51,272 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,434 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 363,774 | 408,865 | 0.8897× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,544 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,861 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,633 | 19,721 | 0.9448× | 0.93× | ✅ |
| socket | 19,873 | 21,036 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,838 | 24,662 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,495 | 25,253 | 0.9304× | 0.93× | ✅ |
| curl | 86,146 | 90,539 | 0.9515× | 0.95× | ✅ |
| dns | 229,669 | 232,909 | 0.9861× | 0.99× | ✅ |
| file | 109,099 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
