# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0fea4db` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,769 | 15,214 | 0.9708× | 0.95× | ✅ |
| echo-1k | 17,955 | 18,167 | 0.9883× | 0.96× | ✅ |
| json | 22,808 | 23,178 | 0.9841× | 0.97× | ✅ |
| service-health | 17,362 | 17,795 | 0.9757× | 0.96× | ✅ |
| websocket | 6,952 | 8,188 | 0.8491× | 0.81× | ✅ |
| timer | 201,673 | 313,305 | 0.6437× | 0.65× | ✅ |
| app | 127,284 | 135,952 | 0.9362× | 0.92× | ✅ |
| http2 | 20,459 | 20,464 | 0.9997× | 0.97× | ✅ |
| table | 197,812 | 202,907 | 0.9749× | 0.98× | ✅ |
| channel | 137,906 | 155,192 | 0.8886× | 0.89× | ✅ |
| task | 32,832 | 33,267 | 0.9869× | 0.96× | ✅ |
| mysql | 50,425 | 51,079 | 0.9872× | 0.97× | ✅ |
| http-client | 34,893 | 35,820 | 0.9741× | 0.94× | ✅ |
| client | 19,303 | 19,775 | 0.9761× | 0.95× | ✅ |
| socket | 20,805 | 21,091 | 0.9865× | 0.99× | ✅ |
| gzip | 197,121 | 197,095 | 1.0001× | 1.02× | ➖ |
| websocket-deflate | 181,747 | 181,564 | 1.0010× | 1.0× | ❌ |
| tcp-length | 5,476 | 4,746 | 1.1537× | 1.07× | ❌ |
| tcp-eof | 5,898 | 10,212 | 0.5775× | 0.55× | ✅ |
| udp | 7,373 | 7,384 | 0.9986× | 1.04× | ✅ |
| connect | 21,660 | 18,707 | 1.1579× | 1.09× | ❌ |
| https | 110,164 | 108,807 | 1.0125× | 1.01× | ❌ |
| wss | 105,373 | 104,794 | 1.0055× | 1.0× | ❌ |
| atomic | 53,216 | 51,334 | 1.0367× | 1.06× | ➖ |
| process-mode | 17,188 | 17,010 | 1.0105× | 0.98× | ❌ |
| http2-concurrent | 444,161 | 408,816 | 1.0865× | 0.93× | ❌ |
| websocket-64k | 156,399 | 159,696 | 0.9794× | 0.62× | ✅ |
| websocket-fragmented | 14,183 | 16,194 | 0.8758× | 0.78× | ✅ |
| upload | 674,839 | 1,322,437 | 0.5103× | 1.05× | ✅ |
| static | 40,925 | 41,057 | 0.9968× | 0.96× | ✅ |

Geometric mean 0.9378×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
