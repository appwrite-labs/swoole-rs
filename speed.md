# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `30c0468` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,004 | 313,794 | 0.6437× | 0.65× | ✅ |
| app | 127,613 | 135,879 | 0.9392× | 0.93× | ✅ |
| http2 | 18,970 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,082 | 33,272 | 0.9642× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,625 | 35,793 | 0.9394× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,589 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,276 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,413 | 17,025 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,656 | 408,865 | 0.8919× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7286× | 0.72× | ✅ |
| upload | 416,528 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 40,999 | 0.6304× | 0.64× | ✅ |
| client | 18,621 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,857 | 21,036 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,820 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,130 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 229,888 | 232,767 | 0.9876× | 0.99× | ✅ |
| file | 109,088 | 131,297 | 0.8308× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
