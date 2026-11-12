# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `39b866e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,919 | 15,210 | 0.9151× | 0.92× | ✅ |
| echo-1k | 17,021 | 18,165 | 0.9370× | 0.94× | ✅ |
| json | 22,018 | 23,161 | 0.9507× | 0.95× | ✅ |
| service-health | 16,513 | 17,773 | 0.9291× | 0.93× | ✅ |
| websocket | 6,565 | 8,191 | 0.8015× | 0.82× | ✅ |
| timer | 203,398 | 313,681 | 0.6484× | 0.65× | ✅ |
| app | 125,865 | 135,936 | 0.9259× | 0.93× | ✅ |
| http2 | 18,721 | 20,468 | 0.9146× | 0.93× | ✅ |
| table | 195,712 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,861 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 32,007 | 33,264 | 0.9622× | 0.96× | ✅ |
| mysql | 49,582 | 51,056 | 0.9711× | 0.97× | ✅ |
| http-client | 33,668 | 35,809 | 0.9402× | 0.94× | ✅ |
| client | 18,462 | 19,757 | 0.9345× | 0.94× | ✅ |
| socket | 19,584 | 21,067 | 0.9296× | 0.92× | ✅ |
| gzip | 196,007 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,332 | 181,572 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,761 | 4,763 | 0.9995× | 1.05× | ✅ |
| tcp-eof | 5,308 | 10,231 | 0.5188× | 0.54× | ✅ |
| udp | 7,543 | 7,987 | 0.9445× | 0.95× | ✅ |
| connect | 18,206 | 18,707 | 0.9733× | 0.98× | ✅ |
| https | 108,756 | 108,810 | 0.9995× | 1.0× | ✅ |
| wss | 104,445 | 104,797 | 0.9966× | 1.0× | ✅ |
| atomic | 49,667 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,703 | 16,959 | 0.9849× | 0.97× | ✅ |
| http2-concurrent | 359,588 | 408,886 | 0.8794× | 0.88× | ✅ |
| websocket-64k | 156,009 | 159,696 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,804 | 16,328 | 0.7842× | 0.79× | ✅ |
| upload | 478,425 | 1,322,387 | 0.3618× | 0.37× | ✅ |
| static | 29,139 | 40,981 | 0.7110× | 0.72× | ✅ |
| pgsql | 23,192 | 24,595 | 0.9430× | 0.95× | ✅ |
| sqlite | 23,456 | 25,236 | 0.9295× | 0.94× | ✅ |
| curl | 84,730 | 90,354 | 0.9378× | 0.94× | ✅ |
| dns | 230,988 | 232,961 | 0.9915× | 0.99× | ✅ |

Geometric mean 0.8821×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
