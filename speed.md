# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a18092e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,918 | 15,214 | 0.9148× | 0.92× | ✅ |
| echo-1k | 17,040 | 18,165 | 0.9380× | 0.94× | ✅ |
| json | 22,008 | 23,161 | 0.9502× | 0.96× | ✅ |
| service-health | 16,505 | 17,773 | 0.9286× | 0.93× | ✅ |
| websocket | 6,719 | 8,188 | 0.8206× | 0.82× | ✅ |
| timer | 202,863 | 314,288 | 0.6455× | 0.65× | ✅ |
| app | 125,978 | 135,920 | 0.9269× | 0.93× | ✅ |
| http2 | 19,833 | 20,464 | 0.9692× | 0.97× | ✅ |
| table | 195,717 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,862 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 31,947 | 33,264 | 0.9604× | 0.96× | ✅ |
| mysql | 49,565 | 51,061 | 0.9707× | 0.97× | ✅ |
| http-client | 33,602 | 35,805 | 0.9385× | 0.94× | ✅ |
| client | 18,461 | 19,753 | 0.9346× | 0.94× | ✅ |
| socket | 19,736 | 21,067 | 0.9369× | 0.92× | ✅ |
| gzip | 196,002 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,474 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,038 | 7,987 | 1.0064× | 1.0× | ❌ |
| connect | 18,255 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,768 | 108,803 | 0.9997× | 1.01× | ✅ |
| wss | 104,594 | 104,794 | 0.9981× | 1.0× | ✅ |
| atomic | 49,664 | 51,316 | 0.9678× | 0.97× | ✅ |
| process-mode | 16,427 | 16,959 | 0.9686× | 0.97× | ✅ |
| http2-concurrent | 372,624 | 408,877 | 0.9113× | 0.91× | ✅ |
| websocket-64k | 156,166 | 159,696 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,076 | 16,331 | 0.8007× | 0.79× | ✅ |
| upload | 544,645 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,448 | 41,045 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,200 | 24,593 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,490 | 25,232 | 0.9310× | 0.94× | ✅ |
| curl | 85,325 | 90,273 | 0.9452× | 0.95× | ✅ |
| dns | 230,544 | 232,679 | 0.9908× | 1.0× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
