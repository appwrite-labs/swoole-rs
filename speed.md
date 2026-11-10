# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ad75691` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,803 | 15,198 | 0.9082× | 0.91× | ✅ |
| echo-1k | 16,686 | 18,159 | 0.9189× | 0.92× | ✅ |
| json | 21,752 | 23,167 | 0.9389× | 0.94× | ✅ |
| service-health | 16,415 | 17,792 | 0.9226× | 0.93× | ✅ |
| websocket | 6,604 | 8,188 | 0.8065× | 0.81× | ✅ |
| timer | 222,795 | 311,267 | 0.7158× | 0.72× | ✅ |
| app | 125,425 | 136,077 | 0.9217× | 0.92× | ✅ |
| http2 | 19,342 | 20,364 | 0.9498× | 0.95× | ✅ |
| table | 196,703 | 203,138 | 0.9683× | 0.99× | ✅ |
| channel | 136,550 | 154,638 | 0.8830× | 0.89× | ✅ |
| task | 31,139 | 33,180 | 0.9385× | 0.94× | ✅ |
| mysql | 49,047 | 51,080 | 0.9602× | 0.97× | ✅ |

Geometric mean 0.8998×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
