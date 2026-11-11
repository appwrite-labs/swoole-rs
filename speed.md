# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ffab504` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,648 | 15,210 | 0.9631× | 0.97× | ✅ |
| echo-1k | 17,847 | 18,175 | 0.9819× | 0.99× | ✅ |
| json | 22,684 | 23,161 | 0.9794× | 0.98× | ✅ |
| service-health | 17,238 | 17,773 | 0.9699× | 0.98× | ✅ |
| websocket | 6,856 | 8,191 | 0.8370× | 0.84× | ✅ |
| timer | 203,033 | 313,419 | 0.6478× | 0.65× | ✅ |
| app | 127,008 | 135,936 | 0.9343× | 0.94× | ✅ |
| http2 | 20,223 | 20,464 | 0.9882× | 0.99× | ✅ |
| table | 196,587 | 201,865 | 0.9738× | 0.98× | ✅ |
| channel | 139,425 | 154,655 | 0.9015× | 0.91× | ✅ |
| task | 32,643 | 33,270 | 0.9811× | 0.99× | ✅ |
| mysql | 50,221 | 51,061 | 0.9835× | 0.99× | ✅ |
| http-client | 34,686 | 35,812 | 0.9686× | 0.97× | ✅ |
| client | 19,153 | 19,757 | 0.9694× | 0.98× | ✅ |
| socket | 20,615 | 21,062 | 0.9788× | 0.98× | ✅ |
| gzip | 196,725 | 197,100 | 0.9981× | 1.0× | ✅ |
| websocket-deflate | 181,637 | 181,580 | 1.0003× | 1.0× | ❌ |
| tcp-length | 5,346 | 4,743 | 1.1272× | 1.14× | ➖ |
| tcp-eof | 5,840 | 10,209 | 0.5721× | 0.58× | ✅ |
| udp | 7,678 | 7,349 | 1.0447× | 1.01× | ❌ |
| connect | 19,539 | 18,707 | 1.0445× | 1.15× | ➖ |
| https | 109,896 | 108,803 | 1.0100× | 1.01× | ❌ |
| wss | 105,105 | 104,794 | 1.0030× | 1.0× | ❌ |
| atomic | 50,458 | 51,316 | 0.9833× | 1.04× | ✅ |
| process-mode | 16,912 | 16,990 | 0.9954× | 1.0× | ✅ |
| http2-concurrent | 375,728 | 408,880 | 0.9189× | 0.93× | ✅ |
| websocket-64k | 156,304 | 159,691 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 14,214 | 16,194 | 0.8777× | 0.87× | ✅ |
| upload | 674,434 | 1,322,434 | 0.5100× | 0.52× | ✅ |
| static | 35,078 | 41,045 | 0.8546× | 0.9× | ✅ |

Geometric mean 0.9207×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
