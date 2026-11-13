# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ad9a160` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,966 | 15,210 | 0.9182× | 0.92× | ✅ |
| echo-1k | 17,080 | 18,162 | 0.9404× | 0.94× | ✅ |
| json | 22,057 | 23,161 | 0.9523× | 0.95× | ✅ |
| service-health | 16,556 | 17,777 | 0.9313× | 0.93× | ✅ |
| websocket | 6,578 | 8,191 | 0.8030× | 0.81× | ✅ |
| timer | 203,448 | 313,671 | 0.6486× | 0.65× | ✅ |
| app | 125,433 | 135,844 | 0.9234× | 0.93× | ✅ |
| http2 | 18,674 | 20,464 | 0.9125× | 0.92× | ✅ |
| table | 195,526 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,467 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,034 | 33,268 | 0.9629× | 0.96× | ✅ |
| mysql | 49,593 | 51,064 | 0.9712× | 0.97× | ✅ |
| http-client | 33,722 | 35,809 | 0.9417× | 0.94× | ✅ |
| client | 18,502 | 19,757 | 0.9365× | 0.94× | ✅ |
| socket | 19,633 | 21,067 | 0.9319× | 0.92× | ✅ |
| gzip | 194,163 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,383 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,762 | 1.0060× | 1.01× | ➖ |
| tcp-eof | 5,313 | 10,228 | 0.5194× | 0.52× | ✅ |
| udp | 7,576 | 7,987 | 0.9485× | 0.95× | ✅ |
| connect | 18,409 | 18,707 | 0.9841× | 0.98× | ✅ |
| https | 108,801 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,393 | 104,800 | 0.9961× | 1.0× | ✅ |
| atomic | 49,729 | 51,316 | 0.9691× | 0.97× | ✅ |
| process-mode | 15,871 | 16,960 | 0.9358× | 0.94× | ✅ |
| http2-concurrent | 359,512 | 408,882 | 0.8793× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,800 | 16,327 | 0.7840× | 0.79× | ✅ |
| upload | 478,116 | 1,322,381 | 0.3616× | 0.37× | ✅ |
| static | 29,217 | 41,103 | 0.7108× | 0.72× | ✅ |
| pgsql | 23,259 | 24,594 | 0.9457× | 0.95× | ✅ |
| sqlite | 23,528 | 25,236 | 0.9323× | 0.94× | ✅ |
| curl | 84,903 | 90,478 | 0.9384× | 0.94× | ✅ |
| dns | 230,946 | 233,068 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8811×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
