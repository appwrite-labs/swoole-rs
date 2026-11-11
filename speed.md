# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `975fb5e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,315 | 15,213 | 0.9409× | 0.93× | ✅ |
| echo-1k | 17,432 | 18,165 | 0.9596× | 0.95× | ✅ |
| json | 22,418 | 23,161 | 0.9679× | 0.96× | ✅ |
| service-health | 16,916 | 17,777 | 0.9516× | 0.94× | ✅ |
| websocket | 6,727 | 8,188 | 0.8216× | 0.83× | ✅ |
| timer | 202,200 | 313,572 | 0.6448× | 0.65× | ✅ |
| app | 126,470 | 135,962 | 0.9302× | 0.93× | ✅ |
| http2 | 20,092 | 20,468 | 0.9816× | 0.98× | ✅ |
| table | 196,128 | 201,865 | 0.9716× | 0.97× | ✅ |
| channel | 138,556 | 154,654 | 0.8959× | 0.91× | ✅ |
| task | 32,245 | 33,268 | 0.9692× | 0.97× | ✅ |
| mysql | 49,952 | 51,056 | 0.9784× | 0.98× | ✅ |
| http-client | 33,930 | 35,809 | 0.9475× | 0.95× | ✅ |
| client | 18,875 | 19,757 | 0.9554× | 0.95× | ✅ |
| socket | 19,842 | 21,067 | 0.9418× | 0.94× | ✅ |
| gzip | 196,286 | 197,079 | 0.9960× | 1.0× | ✅ |
| websocket-deflate | 181,482 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,217 | 4,762 | 1.0955× | 1.1× | ➖ |
| tcp-eof | 5,748 | 10,231 | 0.5618× | 0.57× | ✅ |
| udp | 7,976 | 7,986 | 0.9987× | 1.0× | ✅ |
| connect | 18,536 | 18,707 | 0.9909× | 0.99× | ✅ |
| https | 109,258 | 108,810 | 1.0041× | 1.01× | ➖ |
| wss | 104,673 | 104,794 | 0.9988× | 1.0× | ✅ |
| atomic | 50,083 | 51,316 | 0.9760× | 0.98× | ✅ |
| process-mode | 16,738 | 16,962 | 0.9868× | 0.98× | ✅ |
| http2-concurrent | 376,617 | 408,880 | 0.9211× | 0.92× | ✅ |
| websocket-64k | 156,176 | 159,699 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 12,991 | 16,327 | 0.7956× | 0.79× | ✅ |
| upload | 544,532 | 1,322,383 | 0.4118× | 0.42× | ✅ |
| static | 34,348 | 41,010 | 0.8376× | 0.85× | ✅ |
| pgsql | 23,715 | 24,594 | 0.9643× | 0.96× | ✅ |
| sqlite | 23,949 | 25,232 | 0.9492× | 0.95× | ✅ |
| curl | 85,758 | 90,512 | 0.9475× | 0.95× | ✅ |
| dns | 231,686 | 233,108 | 0.9939× | 1.0× | ✅ |

Geometric mean 0.9063×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
