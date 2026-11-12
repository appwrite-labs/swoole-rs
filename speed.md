# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a839b0e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,905 | 15,213 | 0.9140× | 0.92× | ✅ |
| echo-1k | 17,024 | 18,165 | 0.9372× | 0.94× | ✅ |
| json | 22,015 | 23,161 | 0.9505× | 0.95× | ✅ |
| service-health | 16,512 | 17,777 | 0.9288× | 0.93× | ✅ |
| websocket | 6,726 | 8,191 | 0.8212× | 0.82× | ✅ |
| timer | 202,805 | 313,583 | 0.6467× | 0.65× | ✅ |
| app | 125,945 | 135,917 | 0.9266× | 0.93× | ✅ |
| http2 | 19,229 | 20,468 | 0.9395× | 0.94× | ✅ |
| table | 195,716 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,861 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 31,928 | 33,265 | 0.9598× | 0.96× | ✅ |
| mysql | 49,570 | 50,988 | 0.9722× | 0.97× | ✅ |
| http-client | 33,605 | 35,809 | 0.9384× | 0.94× | ✅ |
| client | 18,462 | 19,754 | 0.9346× | 0.94× | ✅ |
| socket | 19,581 | 21,067 | 0.9295× | 0.92× | ✅ |
| gzip | 195,997 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,490 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,021 | 4,765 | 1.0536× | 1.05× | ❌ |
| tcp-eof | 5,563 | 10,231 | 0.5437× | 0.54× | ✅ |
| udp | 7,557 | 7,986 | 0.9463× | 1.0× | ✅ |
| connect | 18,239 | 18,707 | 0.9750× | 0.98× | ✅ |
| https | 108,779 | 108,803 | 0.9998× | 1.0× | ✅ |
| wss | 104,637 | 104,797 | 0.9985× | 1.0× | ✅ |
| atomic | 49,667 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,407 | 16,960 | 0.9674× | 0.97× | ✅ |
| http2-concurrent | 361,680 | 408,878 | 0.8846× | 0.89× | ✅ |
| websocket-64k | 156,173 | 159,693 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,331 | 0.7888× | 0.79× | ✅ |
| upload | 544,638 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,447 | 41,045 | 0.8392× | 0.84× | ✅ |
| pgsql | 23,194 | 24,595 | 0.9430× | 0.95× | ✅ |
| sqlite | 23,467 | 25,232 | 0.9301× | 0.94× | ✅ |
| curl | 85,283 | 90,350 | 0.9439× | 0.95× | ✅ |
| dns | 230,308 | 232,708 | 0.9897× | 1.0× | ✅ |

Geometric mean 0.8938×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
