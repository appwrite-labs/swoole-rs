# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `83c55af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,213 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,164 | 0.9538× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,021 | 313,839 | 0.6437× | 0.65× | ✅ |
| app | 127,650 | 136,012 | 0.9385× | 0.93× | ✅ |
| http2 | 18,970 | 20,499 | 0.9254× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,641 | 35,793 | 0.9399× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,163 | 181,570 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,589 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,209 | 51,276 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,438 | 17,021 | 0.9070× | 0.89× | ✅ |
| http2-concurrent | 364,437 | 408,861 | 0.8913× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,531 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,890 | 41,028 | 0.6310× | 0.64× | ✅ |
| client | 18,637 | 19,720 | 0.9450× | 0.93× | ✅ |
| socket | 19,870 | 21,036 | 0.9446× | 0.93× | ✅ |
| pgsql | 22,838 | 24,663 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,540 | 25,268 | 0.9316× | 0.93× | ✅ |
| curl | 86,144 | 90,550 | 0.9513× | 0.95× | ✅ |
| dns | 229,960 | 232,840 | 0.9876× | 0.99× | ✅ |
| file | 109,109 | 131,300 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
