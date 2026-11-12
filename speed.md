# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5d07678` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,940 | 15,210 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,043 | 18,162 | 0.9384× | 0.94× | ✅ |
| json | 22,046 | 23,161 | 0.9518× | 0.95× | ✅ |
| service-health | 16,541 | 17,773 | 0.9307× | 0.93× | ✅ |
| websocket | 6,632 | 8,191 | 0.8096× | 0.82× | ✅ |
| timer | 203,473 | 313,484 | 0.6491× | 0.65× | ✅ |
| app | 125,818 | 135,932 | 0.9256× | 0.93× | ✅ |
| http2 | 18,642 | 20,463 | 0.9110× | 0.93× | ✅ |
| table | 195,729 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,874 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 32,313 | 33,264 | 0.9714× | 0.96× | ✅ |
| mysql | 49,606 | 51,064 | 0.9714× | 0.97× | ✅ |
| http-client | 33,670 | 35,809 | 0.9403× | 0.94× | ✅ |
| client | 18,473 | 19,757 | 0.9350× | 0.94× | ✅ |
| socket | 19,590 | 21,067 | 0.9299× | 0.92× | ✅ |
| gzip | 196,023 | 197,083 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,391 | 181,568 | 0.9990× | 1.0× | ✅ |
| tcp-length | 4,839 | 4,762 | 1.0161× | 1.05× | ➖ |
| tcp-eof | 5,387 | 10,228 | 0.5267× | 0.54× | ✅ |
| udp | 7,618 | 7,986 | 0.9539× | 0.95× | ✅ |
| connect | 18,430 | 18,707 | 0.9852× | 0.98× | ✅ |
| https | 108,775 | 108,813 | 0.9996× | 1.0× | ✅ |
| wss | 104,515 | 104,797 | 0.9973× | 1.0× | ✅ |
| atomic | 49,680 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,738 | 16,959 | 0.9870× | 0.97× | ✅ |
| http2-concurrent | 358,965 | 408,880 | 0.8779× | 0.88× | ✅ |
| websocket-64k | 156,077 | 159,696 | 0.9773× | 0.98× | ✅ |
| websocket-fragmented | 12,866 | 16,327 | 0.7880× | 0.79× | ✅ |
| upload | 478,418 | 1,322,380 | 0.3618× | 0.37× | ✅ |
| static | 29,194 | 41,010 | 0.7119× | 0.72× | ✅ |
| pgsql | 23,171 | 24,594 | 0.9421× | 0.95× | ✅ |
| sqlite | 23,452 | 25,236 | 0.9293× | 0.94× | ✅ |
| curl | 84,717 | 90,354 | 0.9376× | 0.94× | ✅ |
| dns | 230,887 | 232,694 | 0.9922× | 0.99× | ✅ |

Geometric mean 0.8843×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
