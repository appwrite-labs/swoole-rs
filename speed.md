# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `69f4877` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,966 | 15,214 | 0.9180× | 0.92× | ✅ |
| echo-1k | 17,080 | 18,162 | 0.9404× | 0.94× | ✅ |
| json | 22,066 | 23,158 | 0.9529× | 0.95× | ✅ |
| service-health | 16,565 | 17,777 | 0.9319× | 0.93× | ✅ |
| websocket | 6,581 | 8,188 | 0.8038× | 0.81× | ✅ |
| timer | 203,460 | 313,586 | 0.6488× | 0.65× | ✅ |
| app | 125,443 | 135,908 | 0.9230× | 0.93× | ✅ |
| http2 | 18,684 | 20,469 | 0.9128× | 0.92× | ✅ |
| table | 195,535 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,473 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,039 | 33,265 | 0.9632× | 0.96× | ✅ |
| mysql | 49,539 | 51,061 | 0.9702× | 0.97× | ✅ |
| http-client | 33,734 | 35,806 | 0.9421× | 0.94× | ✅ |
| client | 18,515 | 19,757 | 0.9371× | 0.94× | ✅ |
| socket | 19,642 | 21,067 | 0.9324× | 0.92× | ✅ |
| gzip | 194,163 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,383 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,765 | 1.0053× | 1.01× | ➖ |
| tcp-eof | 5,314 | 10,228 | 0.5195× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,409 | 18,703 | 0.9843× | 0.98× | ✅ |
| https | 108,809 | 108,810 | 1.0000× | 1.0× | ✅ |
| wss | 104,393 | 104,797 | 0.9961× | 1.0× | ✅ |
| atomic | 49,742 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,877 | 16,962 | 0.9360× | 0.94× | ✅ |
| http2-concurrent | 359,430 | 408,885 | 0.8791× | 0.88× | ✅ |
| websocket-64k | 156,029 | 159,695 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,800 | 16,327 | 0.7840× | 0.79× | ✅ |
| upload | 478,119 | 1,322,380 | 0.3616× | 0.37× | ✅ |
| static | 29,217 | 41,103 | 0.7108× | 0.72× | ✅ |
| pgsql | 23,237 | 24,595 | 0.9448× | 0.95× | ✅ |
| sqlite | 23,510 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 84,886 | 90,319 | 0.9398× | 0.94× | ✅ |
| dns | 230,742 | 232,665 | 0.9917× | 0.99× | ✅ |

Geometric mean 0.8812×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
