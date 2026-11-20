# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7c8731e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,096 | 23,164 | 0.9539× | 0.95× | ✅ |
| service-health | 16,354 | 17,773 | 0.9202× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,827 | 313,767 | 0.6432× | 0.65× | ✅ |
| app | 127,628 | 135,931 | 0.9389× | 0.93× | ✅ |
| http2 | 18,958 | 20,500 | 0.9248× | 0.92× | ✅ |
| table | 193,875 | 202,900 | 0.9555× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,085 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,325 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,646 | 35,792 | 0.9400× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,534 | 18,734 | 0.9360× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,206 | 51,276 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,417 | 17,028 | 0.9054× | 0.89× | ✅ |
| http2-concurrent | 363,802 | 408,865 | 0.8898× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,531 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,954 | 41,092 | 0.6316× | 0.64× | ✅ |
| client | 18,634 | 19,721 | 0.9449× | 0.93× | ✅ |
| socket | 19,873 | 21,036 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,830 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,137 | 90,421 | 0.9526× | 0.95× | ✅ |
| dns | 230,162 | 232,918 | 0.9882× | 0.99× | ✅ |
| file | 109,101 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
