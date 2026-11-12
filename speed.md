# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2dc3139` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,942 | 15,210 | 0.9166× | 0.92× | ✅ |
| echo-1k | 17,045 | 18,162 | 0.9385× | 0.94× | ✅ |
| json | 22,048 | 23,158 | 0.9521× | 0.95× | ✅ |
| service-health | 16,543 | 17,773 | 0.9308× | 0.93× | ✅ |
| websocket | 6,579 | 8,191 | 0.8032× | 0.82× | ✅ |
| timer | 203,406 | 313,655 | 0.6485× | 0.65× | ✅ |
| app | 125,842 | 135,941 | 0.9257× | 0.93× | ✅ |
| http2 | 18,664 | 20,463 | 0.9121× | 0.93× | ✅ |
| table | 195,746 | 201,865 | 0.9697× | 0.97× | ✅ |
| channel | 138,897 | 154,654 | 0.8981× | 0.9× | ✅ |
| task | 32,167 | 33,265 | 0.9670× | 0.96× | ✅ |
| mysql | 49,615 | 51,064 | 0.9716× | 0.97× | ✅ |
| http-client | 33,722 | 35,805 | 0.9418× | 0.94× | ✅ |
| client | 18,496 | 19,757 | 0.9362× | 0.94× | ✅ |
| socket | 19,615 | 21,067 | 0.9311× | 0.92× | ✅ |
| gzip | 196,024 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,339 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,773 | 4,765 | 1.0015× | 1.05× | ➖ |
| tcp-eof | 5,321 | 10,228 | 0.5202× | 0.54× | ✅ |
| udp | 7,567 | 7,986 | 0.9475× | 0.95× | ✅ |
| connect | 18,368 | 18,707 | 0.9819× | 0.98× | ✅ |
| https | 108,804 | 108,803 | 1.0000× | 1.0× | ❌ |
| wss | 104,469 | 104,797 | 0.9969× | 1.0× | ✅ |
| atomic | 49,697 | 51,316 | 0.9685× | 0.97× | ✅ |
| process-mode | 16,719 | 16,959 | 0.9858× | 0.99× | ✅ |
| http2-concurrent | 359,463 | 408,880 | 0.8791× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,692 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,810 | 16,331 | 0.7844× | 0.79× | ✅ |
| upload | 478,448 | 1,322,381 | 0.3618× | 0.37× | ✅ |
| static | 29,193 | 40,997 | 0.7121× | 0.72× | ✅ |
| pgsql | 23,218 | 24,595 | 0.9440× | 0.95× | ✅ |
| sqlite | 23,500 | 25,236 | 0.9312× | 0.94× | ✅ |
| curl | 84,604 | 90,479 | 0.9351× | 0.94× | ✅ |
| dns | 230,716 | 232,609 | 0.9919× | 0.99× | ✅ |

Geometric mean 0.8831×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
