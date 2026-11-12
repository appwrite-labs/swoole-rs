# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5eeb72f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,934 | 15,213 | 0.9159× | 0.92× | ✅ |
| echo-1k | 17,038 | 18,165 | 0.9379× | 0.94× | ✅ |
| json | 22,031 | 23,161 | 0.9512× | 0.95× | ✅ |
| service-health | 16,526 | 17,777 | 0.9296× | 0.93× | ✅ |
| websocket | 6,574 | 8,188 | 0.8029× | 0.82× | ✅ |
| timer | 203,366 | 313,630 | 0.6484× | 0.65× | ✅ |
| app | 125,815 | 135,914 | 0.9257× | 0.93× | ✅ |
| http2 | 18,652 | 20,464 | 0.9115× | 0.93× | ✅ |
| table | 195,736 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,887 | 154,651 | 0.8981× | 0.9× | ✅ |
| task | 32,119 | 33,264 | 0.9656× | 0.96× | ✅ |
| mysql | 49,591 | 51,060 | 0.9712× | 0.97× | ✅ |
| http-client | 33,673 | 35,802 | 0.9405× | 0.94× | ✅ |
| client | 18,479 | 19,757 | 0.9353× | 0.94× | ✅ |
| socket | 19,597 | 21,067 | 0.9302× | 0.92× | ✅ |
| gzip | 196,021 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,337 | 181,572 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,769 | 4,765 | 1.0007× | 1.05× | ➖ |
| tcp-eof | 5,317 | 10,231 | 0.5196× | 0.54× | ✅ |
| udp | 7,559 | 7,986 | 0.9465× | 0.95× | ✅ |
| connect | 18,368 | 18,707 | 0.9819× | 0.98× | ✅ |
| https | 108,798 | 108,813 | 0.9999× | 1.0× | ✅ |
| wss | 104,461 | 104,797 | 0.9968× | 1.0× | ✅ |
| atomic | 49,680 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,678 | 17,024 | 0.9796× | 0.99× | ✅ |
| http2-concurrent | 359,360 | 408,881 | 0.8789× | 0.88× | ✅ |
| websocket-64k | 156,015 | 159,698 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,327 | 0.7847× | 0.79× | ✅ |
| upload | 478,432 | 1,322,383 | 0.3618× | 0.37× | ✅ |
| static | 29,175 | 40,981 | 0.7119× | 0.72× | ✅ |
| pgsql | 23,242 | 24,595 | 0.9450× | 0.95× | ✅ |
| sqlite | 23,510 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 84,619 | 90,522 | 0.9348× | 0.94× | ✅ |
| dns | 230,990 | 232,743 | 0.9925× | 0.99× | ✅ |

Geometric mean 0.8826×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
