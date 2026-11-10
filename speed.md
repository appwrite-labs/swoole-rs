# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5c6c42f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,772 | 15,214 | 0.9710× | 0.95× | ✅ |
| echo-1k | 17,943 | 18,168 | 0.9877× | 0.96× | ✅ |
| json | 22,803 | 23,178 | 0.9838× | 0.97× | ✅ |
| service-health | 17,359 | 17,795 | 0.9755× | 0.96× | ✅ |
| websocket | 6,949 | 8,188 | 0.8487× | 0.81× | ✅ |
| timer | 201,525 | 313,424 | 0.6430× | 0.65× | ✅ |
| app | 127,705 | 135,946 | 0.9394× | 0.92× | ✅ |
| http2 | 20,449 | 20,467 | 0.9991× | 0.97× | ✅ |
| table | 197,734 | 202,907 | 0.9745× | 0.98× | ✅ |
| channel | 140,372 | 155,192 | 0.9045× | 0.89× | ✅ |
| task | 32,849 | 33,281 | 0.9870× | 0.96× | ✅ |
| mysql | 50,412 | 51,079 | 0.9869× | 0.97× | ✅ |
| http-client | 34,898 | 35,821 | 0.9742× | 0.94× | ✅ |
| client | 19,302 | 19,775 | 0.9761× | 0.95× | ✅ |
| socket | 20,806 | 21,090 | 0.9865× | 0.99× | ✅ |
| gzip | 197,116 | 197,088 | 1.0001× | 1.02× | ➖ |
| websocket-deflate | 181,743 | 181,560 | 1.0010× | 1.0× | ❌ |
| tcp-length | 5,475 | 4,746 | 1.1535× | 1.07× | ❌ |
| tcp-eof | 5,898 | 10,212 | 0.5775× | 0.55× | ✅ |
| udp | 7,373 | 7,329 | 1.0060× | 1.04× | ➖ |
| connect | 21,673 | 18,707 | 1.1585× | 1.09× | ❌ |
| https | 110,163 | 108,810 | 1.0124× | 1.01× | ❌ |
| wss | 105,358 | 104,801 | 1.0053× | 1.0× | ❌ |
| atomic | 53,216 | 51,330 | 1.0368× | 1.06× | ➖ |
| process-mode | 17,185 | 16,945 | 1.0142× | 0.98× | ❌ |
| http2-concurrent | 444,056 | 408,813 | 1.0862× | 0.93× | ❌ |
| websocket-64k | 156,404 | 159,693 | 0.9794× | 0.62× | ✅ |
| websocket-fragmented | 14,176 | 16,195 | 0.8754× | 0.78× | ✅ |
| upload | 675,739 | 1,322,416 | 0.5110× | 1.05× | ✅ |
| static | 40,908 | 40,999 | 0.9978× | 0.96× | ✅ |

Geometric mean 0.9387×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
