# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `84ef856` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,915 | 15,210 | 0.9149× | 0.92× | ✅ |
| echo-1k | 17,021 | 18,162 | 0.9372× | 0.94× | ✅ |
| json | 22,019 | 23,158 | 0.9508× | 0.95× | ✅ |
| service-health | 16,513 | 17,773 | 0.9291× | 0.93× | ✅ |
| websocket | 6,569 | 8,191 | 0.8019× | 0.82× | ✅ |
| timer | 203,382 | 313,615 | 0.6485× | 0.65× | ✅ |
| app | 125,876 | 135,923 | 0.9261× | 0.93× | ✅ |
| http2 | 18,724 | 20,463 | 0.9150× | 0.93× | ✅ |
| table | 195,716 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,864 | 154,651 | 0.8979× | 0.9× | ✅ |
| task | 32,004 | 33,264 | 0.9621× | 0.96× | ✅ |
| mysql | 49,582 | 51,061 | 0.9710× | 0.97× | ✅ |
| http-client | 33,669 | 35,808 | 0.9403× | 0.94× | ✅ |
| client | 18,466 | 19,757 | 0.9347× | 0.94× | ✅ |
| socket | 19,585 | 21,067 | 0.9296× | 0.92× | ✅ |
| gzip | 195,988 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,329 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,761 | 4,762 | 0.9997× | 1.05× | ✅ |
| tcp-eof | 5,308 | 10,231 | 0.5188× | 0.54× | ✅ |
| udp | 7,543 | 7,987 | 0.9444× | 0.95× | ✅ |
| connect | 18,206 | 18,707 | 0.9733× | 0.98× | ✅ |
| https | 108,753 | 108,810 | 0.9995× | 1.0× | ✅ |
| wss | 104,445 | 104,800 | 0.9966× | 1.0× | ✅ |
| atomic | 49,667 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,703 | 16,957 | 0.9851× | 0.97× | ✅ |
| http2-concurrent | 359,566 | 408,880 | 0.8794× | 0.88× | ✅ |
| websocket-64k | 156,013 | 159,693 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,799 | 16,331 | 0.7838× | 0.79× | ✅ |
| upload | 478,426 | 1,322,384 | 0.3618× | 0.37× | ✅ |
| static | 29,168 | 41,010 | 0.7112× | 0.72× | ✅ |
| pgsql | 23,180 | 24,595 | 0.9425× | 0.95× | ✅ |
| sqlite | 23,456 | 25,236 | 0.9295× | 0.94× | ✅ |
| curl | 84,733 | 90,350 | 0.9378× | 0.94× | ✅ |
| dns | 230,379 | 233,396 | 0.9871× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
