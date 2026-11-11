# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `99d6712` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,011 | 15,214 | 0.9210× | 0.93× | ✅ |
| echo-1k | 17,129 | 18,165 | 0.9430× | 0.95× | ✅ |
| json | 22,118 | 23,161 | 0.9550× | 0.96× | ✅ |
| service-health | 16,608 | 17,777 | 0.9343× | 0.94× | ✅ |
| websocket | 6,669 | 8,191 | 0.8142× | 0.82× | ✅ |
| timer | 202,057 | 313,546 | 0.6444× | 0.65× | ✅ |
| app | 125,675 | 135,972 | 0.9243× | 0.93× | ✅ |
| http2 | 19,745 | 20,464 | 0.9649× | 0.98× | ✅ |
| table | 195,818 | 201,865 | 0.9700× | 0.97× | ✅ |
| channel | 138,245 | 154,655 | 0.8939× | 0.9× | ✅ |
| task | 32,007 | 33,265 | 0.9622× | 0.97× | ✅ |
| mysql | 49,519 | 51,064 | 0.9697× | 0.98× | ✅ |
| http-client | 33,511 | 35,809 | 0.9358× | 0.95× | ✅ |
| client | 18,502 | 19,758 | 0.9364× | 0.95× | ✅ |
| socket | 19,472 | 21,067 | 0.9243× | 0.94× | ✅ |
| gzip | 195,982 | 197,079 | 0.9944× | 1.0× | ✅ |
| websocket-deflate | 181,425 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 5,170 | 4,762 | 1.0858× | 1.09× | ➖ |
| tcp-eof | 5,705 | 10,228 | 0.5578× | 0.56× | ✅ |
| udp | 7,918 | 7,986 | 0.9915× | 1.0× | ✅ |
| connect | 18,143 | 18,707 | 0.9699× | 0.99× | ✅ |
| https | 108,880 | 108,803 | 1.0007× | 1.01× | ➖ |
| wss | 104,583 | 104,801 | 0.9979× | 1.0× | ✅ |
| atomic | 49,769 | 51,316 | 0.9699× | 0.98× | ✅ |
| process-mode | 16,418 | 16,963 | 0.9679× | 0.98× | ✅ |
| http2-concurrent | 372,444 | 408,964 | 0.9107× | 0.92× | ✅ |
| websocket-64k | 156,114 | 159,691 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 12,922 | 16,331 | 0.7912× | 0.79× | ✅ |
| upload | 544,216 | 1,322,381 | 0.4115× | 0.42× | ✅ |
| static | 34,185 | 41,068 | 0.8324× | 0.84× | ✅ |
| pgsql | 23,265 | 24,595 | 0.9459× | 0.96× | ✅ |
| sqlite | 23,549 | 25,232 | 0.9333× | 0.95× | ✅ |
| curl | 85,252 | 90,504 | 0.9420× | 0.95× | ✅ |
| dns | 231,094 | 233,236 | 0.9908× | 1.0× | ✅ |

Geometric mean 0.8978×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
