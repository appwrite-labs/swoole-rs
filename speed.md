# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be7677e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,020 | 313,714 | 0.6440× | 0.65× | ✅ |
| app | 127,616 | 135,947 | 0.9387× | 0.93× | ✅ |
| http2 | 18,956 | 20,500 | 0.9247× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,628 | 35,793 | 0.9395× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,764 | 0.9784× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,802 | 0.9771× | 0.98× | ✅ |
| wss | 101,589 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,416 | 17,028 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,174 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,527 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,623 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,042 | 0.9438× | 0.93× | ✅ |
| pgsql | 22,825 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,131 | 90,506 | 0.9517× | 0.95× | ✅ |
| dns | 230,097 | 232,636 | 0.9891× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
