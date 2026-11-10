# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d349412` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,910 | 15,214 | 1.6374× | 0.95× | ❌ |
| echo-1k | 32,663 | 18,174 | 1.7973× | 0.96× | ❌ |
| json | 33,484 | 23,178 | 1.4447× | 0.97× | ❌ |
| service-health | 27,702 | 17,795 | 1.5567× | 0.96× | ❌ |
| websocket | 6,606 | 8,188 | 0.8068× | 0.81× | ✅ |
| timer | 211,865 | 313,420 | 0.6760× | 0.65× | ✅ |
| app | 140,828 | 135,948 | 1.0359× | 0.92× | ❌ |
| http2 | 19,660 | 20,467 | 0.9606× | 0.97× | ✅ |
| table | 208,115 | 202,907 | 1.0257× | 0.98× | ❌ |
| channel | 148,288 | 155,192 | 0.9555× | 0.89× | ✅ |
| task | 46,966 | 33,266 | 1.4118× | 0.96× | ❌ |
| mysql | 60,098 | 51,074 | 1.1767× | 0.97× | ❌ |
| http-client | 52,562 | 35,824 | 1.4672× | 0.94× | ❌ |
| client | 29,373 | 19,774 | 1.4854× | 0.95× | ❌ |
| socket | 30,855 | 21,090 | 1.4630× | 0.99× | ❌ |
| gzip | 215,840 | 197,092 | 1.0951× | 1.02× | ❌ |
| websocket-deflate | 181,404 | 181,560 | 0.9991× | 1.0× | ✅ |
| tcp-length | 5,074 | 4,743 | 1.0699× | 1.07× | ➖ |
| tcp-eof | 5,585 | 10,212 | 0.5469× | 0.55× | ✅ |
| udp | 7,283 | 7,345 | 0.9916× | 1.07× | ✅ |
| connect | 33,484 | 18,703 | 1.7903× | 1.09× | ❌ |
| https | 119,691 | 108,803 | 1.1001× | 1.01× | ❌ |
| wss | 104,295 | 104,797 | 0.9952× | 1.0× | ✅ |
| atomic | 64,711 | 51,334 | 1.2606× | 1.06× | ❌ |
| process-mode | 27,035 | 16,951 | 1.5948× | 0.98× | ❌ |
| http2-concurrent | 438,882 | 408,817 | 1.0735× | 0.93× | ❌ |
| websocket-64k | 156,053 | 159,697 | 0.9772× | 0.62× | ✅ |
| websocket-fragmented | 13,527 | 16,195 | 0.8353× | 0.78× | ✅ |
| upload | 1,799,922 | 1,322,459 | 1.3610× | 1.05× | ❌ |
| static | 39,233 | 41,028 | 0.9562× | 0.96× | ✅ |

Geometric mean 1.1416×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
