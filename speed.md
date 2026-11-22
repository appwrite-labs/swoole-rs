# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `95bee77` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,012 | 313,754 | 0.6439× | 0.65× | ✅ |
| app | 127,631 | 135,897 | 0.9392× | 0.93× | ✅ |
| http2 | 18,957 | 20,500 | 0.9247× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,086 | 0.9848× | 0.97× | ✅ |
| http-client | 33,658 | 35,796 | 0.9403× | 0.93× | ✅ |
| gzip | 194,287 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,276 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9054× | 0.89× | ✅ |
| http2-concurrent | 364,519 | 408,867 | 0.8915× | 0.89× | ✅ |
| websocket-64k | 90,009 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,525 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,623 | 19,720 | 0.9444× | 0.93× | ✅ |
| socket | 19,860 | 21,036 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,818 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,129 | 90,417 | 0.9526× | 0.95× | ✅ |
| dns | 230,377 | 232,724 | 0.9899× | 0.99× | ✅ |
| file | 109,088 | 131,207 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
