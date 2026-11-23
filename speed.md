# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `939da19` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,092 | 23,164 | 0.9537× | 0.95× | ✅ |
| service-health | 16,347 | 17,777 | 0.9195× | 0.92× | ✅ |
| websocket | 6,721 | 8,189 | 0.8208× | 0.81× | ✅ |
| timer | 201,915 | 313,782 | 0.6435× | 0.65× | ✅ |
| app | 127,646 | 135,919 | 0.9391× | 0.93× | ✅ |
| http2 | 18,968 | 20,493 | 0.9256× | 0.92× | ✅ |
| table | 194,383 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,371 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,079 | 33,269 | 0.9642× | 0.96× | ✅ |
| mysql | 50,320 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,638 | 35,796 | 0.9397× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,524 | 18,730 | 0.9356× | 0.93× | ✅ |
| https | 106,313 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,205 | 51,276 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 363,916 | 408,866 | 0.8901× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,528 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,877 | 40,999 | 0.6312× | 0.64× | ✅ |
| client | 18,630 | 19,718 | 0.9448× | 0.93× | ✅ |
| socket | 19,866 | 21,037 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,818 | 24,663 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,516 | 25,253 | 0.9312× | 0.93× | ✅ |
| curl | 86,122 | 90,549 | 0.9511× | 0.95× | ✅ |
| dns | 229,804 | 232,556 | 0.9882× | 0.99× | ✅ |
| file | 109,102 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
