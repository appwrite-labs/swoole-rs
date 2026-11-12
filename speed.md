# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c14ff2a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,008 | 15,210 | 0.9210× | 0.92× | ✅ |
| echo-1k | 17,121 | 18,165 | 0.9425× | 0.94× | ✅ |
| json | 22,112 | 23,161 | 0.9547× | 0.95× | ✅ |
| service-health | 16,607 | 17,777 | 0.9342× | 0.93× | ✅ |
| websocket | 6,729 | 8,191 | 0.8214× | 0.82× | ✅ |
| timer | 203,569 | 314,788 | 0.6467× | 0.65× | ✅ |
| app | 125,949 | 135,888 | 0.9269× | 0.93× | ✅ |
| http2 | 18,939 | 20,467 | 0.9253× | 0.94× | ✅ |
| table | 195,807 | 201,869 | 0.9700× | 0.97× | ✅ |
| channel | 138,439 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,015 | 33,265 | 0.9624× | 0.96× | ✅ |
| mysql | 49,656 | 51,064 | 0.9724× | 0.97× | ✅ |
| http-client | 33,713 | 35,807 | 0.9415× | 0.94× | ✅ |
| client | 18,557 | 19,757 | 0.9393× | 0.94× | ✅ |
| socket | 19,673 | 21,063 | 0.9340× | 0.92× | ✅ |
| gzip | 196,080 | 197,083 | 0.9949× | 1.0× | ✅ |
| websocket-deflate | 181,493 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 4,995 | 4,765 | 1.0481× | 1.05× | ➖ |
| tcp-eof | 5,533 | 10,231 | 0.5408× | 0.54× | ✅ |
| udp | 7,545 | 7,987 | 0.9447× | 0.95× | ✅ |
| connect | 18,326 | 18,707 | 0.9797× | 0.98× | ✅ |
| https | 108,839 | 108,803 | 1.0003× | 1.0× | ❌ |
| wss | 104,616 | 104,794 | 0.9983× | 1.0× | ✅ |
| atomic | 49,758 | 51,316 | 0.9696× | 0.97× | ✅ |
| process-mode | 16,564 | 16,959 | 0.9767× | 0.97× | ✅ |
| http2-concurrent | 359,581 | 408,881 | 0.8794× | 0.89× | ✅ |
| websocket-64k | 156,175 | 159,694 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,116 | 16,327 | 0.8033× | 0.79× | ✅ |
| upload | 478,532 | 1,322,381 | 0.3619× | 0.42× | ✅ |
| static | 29,325 | 41,016 | 0.7150× | 0.84× | ✅ |
| pgsql | 23,291 | 24,594 | 0.9470× | 0.95× | ✅ |
| sqlite | 23,557 | 25,232 | 0.9336× | 0.94× | ✅ |
| curl | 85,068 | 90,512 | 0.9399× | 0.95× | ✅ |
| dns | 230,594 | 232,829 | 0.9904× | 0.99× | ✅ |

Geometric mean 0.8873×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
