# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `614f2d2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,233 | 15,210 | 0.9358× | 0.93× | ✅ |
| echo-1k | 17,360 | 18,162 | 0.9558× | 0.95× | ✅ |
| json | 22,337 | 23,162 | 0.9644× | 0.96× | ✅ |
| service-health | 16,836 | 17,773 | 0.9472× | 0.94× | ✅ |
| websocket | 6,766 | 8,191 | 0.8260× | 0.82× | ✅ |
| timer | 202,206 | 313,795 | 0.6444× | 0.65× | ✅ |
| app | 126,359 | 135,878 | 0.9299× | 0.93× | ✅ |
| http2 | 20,050 | 20,467 | 0.9796× | 0.98× | ✅ |
| table | 196,025 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 138,449 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,244 | 33,265 | 0.9693× | 0.97× | ✅ |
| mysql | 49,871 | 51,049 | 0.9769× | 0.98× | ✅ |
| http-client | 33,833 | 35,807 | 0.9449× | 0.95× | ✅ |
| client | 18,770 | 19,756 | 0.9501× | 0.95× | ✅ |
| socket | 19,738 | 21,067 | 0.9369× | 0.94× | ✅ |
| gzip | 196,202 | 197,083 | 0.9955× | 1.0× | ✅ |
| websocket-deflate | 181,525 | 181,572 | 0.9997× | 1.0× | ✅ |
| tcp-length | 5,259 | 4,762 | 1.1043× | 1.09× | ❌ |
| tcp-eof | 5,794 | 10,228 | 0.5664× | 0.56× | ✅ |
| udp | 7,997 | 7,987 | 1.0012× | 1.0× | ❌ |
| connect | 18,469 | 18,707 | 0.9873× | 0.99× | ✅ |
| https | 109,133 | 108,803 | 1.0030× | 1.01× | ➖ |
| wss | 104,743 | 104,794 | 0.9995× | 1.0× | ✅ |
| atomic | 49,976 | 51,316 | 0.9739× | 0.98× | ✅ |
| process-mode | 16,634 | 16,962 | 0.9806× | 0.98× | ✅ |
| http2-concurrent | 375,590 | 409,454 | 0.9173× | 0.92× | ✅ |
| websocket-64k | 156,215 | 159,698 | 0.9782× | 0.98× | ✅ |
| websocket-fragmented | 13,030 | 16,327 | 0.7980× | 0.79× | ✅ |
| upload | 544,412 | 1,322,380 | 0.4117× | 0.42× | ✅ |
| static | 34,331 | 41,010 | 0.8371× | 0.84× | ✅ |
| pgsql | 23,606 | 24,598 | 0.9597× | 0.96× | ✅ |
| sqlite | 23,835 | 25,232 | 0.9446× | 0.95× | ✅ |
| curl | 85,654 | 90,497 | 0.9465× | 0.95× | ✅ |
| dns | 231,627 | 232,882 | 0.9946× | 1.0× | ✅ |

Geometric mean 0.9054×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
