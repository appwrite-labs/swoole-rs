# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0ceecb8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,748 | 15,213 | 0.9037× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,166 | 0.9224× | 0.92× | ✅ |
| json | 22,076 | 23,173 | 0.9527× | 0.95× | ✅ |
| service-health | 16,334 | 17,786 | 0.9184× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,531 | 313,717 | 0.6424× | 0.65× | ✅ |
| app | 127,002 | 135,890 | 0.9346× | 0.93× | ✅ |
| http2 | 18,965 | 20,498 | 0.9252× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,357 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 31,961 | 33,272 | 0.9606× | 0.96× | ✅ |
| mysql | 50,047 | 51,082 | 0.9797× | 0.97× | ✅ |
| http-client | 33,371 | 35,794 | 0.9323× | 0.93× | ✅ |
| gzip | 194,366 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,987 | 0.9466× | 0.95× | ✅ |
| connect | 17,588 | 18,730 | 0.9390× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,800 | 0.9658× | 0.97× | ✅ |
| atomic | 50,187 | 51,276 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,271 | 17,021 | 0.8972× | 0.89× | ✅ |
| http2-concurrent | 365,532 | 408,867 | 0.8940× | 0.89× | ✅ |
| websocket-64k | 89,974 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,368 | 0.7231× | 0.72× | ✅ |
| upload | 416,432 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,109 | 41,063 | 0.6602× | 0.65× | ✅ |
| client | 18,479 | 19,720 | 0.9370× | 0.93× | ✅ |
| socket | 19,704 | 21,033 | 0.9368× | 0.93× | ✅ |
| pgsql | 23,099 | 24,663 | 0.9366× | 0.92× | ✅ |
| sqlite | 23,442 | 25,253 | 0.9283× | 0.93× | ✅ |
| curl | 85,926 | 90,417 | 0.9503× | 0.95× | ✅ |
| dns | 230,208 | 232,977 | 0.9881× | 0.99× | ✅ |
| file | 108,246 | 130,844 | 0.8273× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
