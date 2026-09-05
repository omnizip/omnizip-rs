# 23 — Deflate64 interop probe

- **Priority:** HIGH (correctness of a claimed format)
- **Depends on:** [22](22-extend-fuzz-coverage.md)'s follow-up
- **Status:** done 2026-09-05 — probe ran; finding is bigger than the table

## Method (reproducible)

7-Zip WRITES Deflate64 (`7zz a -mm=Deflate64`), giving a ground-truth
oracle:

```bash
python3 -c "
import random; random.seed(42)
A = bytes(random.randrange(65,123) for _ in range(50000))
B = bytes(random.randrange(65,123) for _ in range(20000))
open('/tmp/d64src.bin','wb').write(A+B+A)"     # A repeats at distance 70000
7zz a -mm=Deflate64 /tmp/d64/d64.zip /tmp/d64src.bin   # method 9 archive
7zz x ...                                               # oracle bytes
# feed the raw member (PK\x03\x04 +30+nlen+elen .. +csize) to
# Deflate64Codec::decompress and compare
```

## Finding

**omnizip-deflate64 does not interoperate with reference Deflate64.**
Our decoder rejects the 7zz stream at the block header
(`literal table length exceeds buffer`) — before the distance-table
question even arises. The codec (a Ruby port) has only ever been
self-consistent: it round-trips its own streams and passes the fuzz
gate, but was never validated against a foreign producer. The 0.21.55
distance-table fix made it internally correct within its own
(dubious) layout; the wire layout itself diverges.

Impact: zipx/method-9 archives from real tools cannot be read
(structured error, no corruption — the decoder fails closed).
Our own method-9 output is likewise only readable by us.

## Hand-off

The wire-true port is [24](24-deflate64-wire-port.md). The fixture
recipe above is the acceptance oracle for it.
