# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `af351fb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,916 | 15,210 | 0.9149× | 0.92× | ✅ |
| echo-1k | 17,022 | 18,162 | 0.9373× | 0.94× | ✅ |
| json | 22,032 | 23,161 | 0.9512× | 0.95× | ✅ |
| service-health | 16,528 | 17,773 | 0.9299× | 0.93× | ✅ |
| websocket | 6,561 | 8,191 | 0.8010× | 0.82× | ✅ |
| timer | 203,477 | 313,557 | 0.6489× | 0.65× | ✅ |
| app | 125,842 | 135,936 | 0.9257× | 0.93× | ✅ |
| http2 | 18,714 | 20,464 | 0.9145× | 0.93× | ✅ |
| table | 195,731 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,879 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 32,005 | 33,268 | 0.9620× | 0.96× | ✅ |
| mysql | 49,591 | 51,050 | 0.9714× | 0.97× | ✅ |
| http-client | 33,684 | 35,806 | 0.9407× | 0.94× | ✅ |
| client | 18,480 | 19,756 | 0.9354× | 0.94× | ✅ |
| socket | 19,599 | 21,067 | 0.9304× | 0.92× | ✅ |
| gzip | 195,985 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,326 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,761 | 4,765 | 0.9990× | 1.05× | ✅ |
| tcp-eof | 5,309 | 10,231 | 0.5188× | 0.54× | ✅ |
| udp | 7,541 | 7,986 | 0.9442× | 0.95× | ✅ |
| connect | 18,194 | 18,707 | 0.9726× | 0.98× | ✅ |
| https | 108,775 | 108,806 | 0.9997× | 1.0× | ✅ |
| wss | 104,449 | 104,800 | 0.9966× | 1.0× | ✅ |
| atomic | 49,682 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,712 | 16,957 | 0.9856× | 0.97× | ✅ |
| http2-concurrent | 359,599 | 408,881 | 0.8795× | 0.88× | ✅ |
| websocket-64k | 156,006 | 159,698 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,801 | 16,328 | 0.7840× | 0.79× | ✅ |
| upload | 478,405 | 1,322,380 | 0.3618× | 0.37× | ✅ |
| static | 29,178 | 40,981 | 0.7120× | 0.72× | ✅ |
| pgsql | 23,197 | 24,594 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,478 | 25,236 | 0.9304× | 0.94× | ✅ |
| curl | 84,748 | 90,479 | 0.9367× | 0.94× | ✅ |
| dns | 230,638 | 233,354 | 0.9884× | 0.99× | ✅ |

Geometric mean 0.8822×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
