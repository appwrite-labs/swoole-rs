"""Defined-input adversarial corpus, independently constructed by the M01 owner."""
import ctypes
import itertools

def generate(unsigned_char=False):
    W=ctypes.sizeof(ctypes.c_ulong)*8;maximum=(1<<W)-1;longmax=(1<<(W-1))-1
    cases=[]
    def add(op,a,b='',locale='C',initial=0,expected=None,note=''):
     cases.append(dict(id=str(len(cases)),operation=op,a=a,b=b,locale=locale,initial_errno=initial,expected_value=expected,note=note))
    for n in range(128):add('type',str(n),expected=next((str(size) for chars,size in [('cC',1),('sSnv',2),('lLNV',4),('qQJP',8)] if chr(n) in chars),'0'))
    # Bound subtraction-based gcd input ratios; no zero, and no multi-billion subtraction runs.
    pairs=[(1,1),(1,257),(257,1),(65535,65536),(65536,65535),(2147483647,2147483646),(4294967295,4294967295),(4294967295,4294967294),(1836311903,1134903170)]
    import math
    for a,b in pairs+[(2147483648,1073741824),(196605,131070)]:
     u,v=a,b;steps=0
     while v:
      q,r=divmod(u,v);steps+=q;u,v=v,r
     if steps>100000:continue
     add('gcd',str(a),str(b),expected=str(math.gcd(a,b)))
    # lcm's int remainder is representable for these; multiplication is explicitly uint32 wrap-before-divide.
    for a,b in pairs+[(65536,65536),(65536,131072),(100000,100000),(4294967295,2),(2,4294967295),(2147483648,2147483648)]:
     # Exclude modulo result > INT_MAX: int conversion implementation-dependent.
     u,v=a,b;valid=True
     while u%v:
      r=u%v
      if r>2147483647:valid=False;break
      u,v=v,r
     if valid:add('lcm',str(a),str(b),expected=str(((a*b)&0xffffffff)//math.gcd(a,b)),note='product-before-divide uint32 wrap')
    def radix(v,b):
     out='';digits='0123456789abcdefghijklmnopqrstuvwxyz'
     while True:
      out=digits[v%b]+out;v//=b
      if not v:return out.encode().hex()
    for base in range(2,37):
     for v in sorted(set([0,1,base-1,base,base+1,base*base-1,base*base,maximum,maximum-1,1<<63])):add('radix',str(v),str(base),expected=radix(v,base))
    ints={0,1,-1,longmax,-longmax,longmax-1,-longmax+1}
    for p in range(1,19):
     for d in [-1,0,1]:
      v=10**p+d
      if v<=longmax:ints.update([v,-v])
    for v in sorted(ints):
     s=str(v).encode();add('decimal',str(v),expected=f'{len(s)}:{s.hex()}:0')
    hexes=[b'',b'0',b'0x',b'0X',b'0xz',b'z',b'g',b'G',b'Zz',b'FFtail',b'ff!',b' 0x10',b'+ff',b'-1',b'1_2',b'1.2',b'0x0x1',b'1\x002',b'0x\x00ff',b'f'*16,b'f'*17,b'z'*256,b'0x'+b'F'*4096]
    hexes += [bytes([n]) for n in range(1,256 if unsigned_char else 128)]
    locales=['C','POSIX','C.UTF-8','en_US.UTF-8','tr_TR.UTF-8']
    for loc in locales:
     for s in hexes:
      ss=s.split(b'\0')[0];i=2 if ss[:2].lower()==b'0x' else 0;v=0
      while i<len(ss):
       c=ss[i];d=c-48 if 48<=c<=57 else (c-65+10 if 65<=c<=90 else (c-97+10 if 97<=c<=122 else -1))
       if d<0:break
       v=(v*16+d)&maximum;i+=1
      add('hex',s.hex(),locale=loc,initial=7,expected=f'{v}:{i}' if loc in ['C','POSIX'] else None,note='ASCII-only toupper inputs; optional prefix counts even with no digits')
    versions=[b'',b'0',b'1',b'01',b'1.0',b'1.0.0',b'1..2',b'1x2',b'1-2',b' 1',b'+1',b'-1',str(maximum).encode(),str(maximum+1).encode(),b'9'*80,b'x',b'.',b'1\x0099',b'1e3',b'1\t2']
    for loc in locales:
     for a,b in itertools.product(versions,repeat=2):
      for initial in [0,7]:add('version',a.hex(),b.hex(),locale=loc,initial=initial,note='libc strtoul oracle; compare result and sticky errno on same platform/locale')
    long_bits=ctypes.sizeof(ctypes.c_long)*8
    long_min=-(1<<(long_bits-1));long_max=(1<<(long_bits-1))-1
    env_values=[b'',b'0',b'+0',b'-0',b'  +42tail',b'\t-2147483648x',b'2147483647',b'2147483648',b'-2147483649',b'x',b'  ',b'+',b'--1',b'12.34',str(long_min).encode(),str(long_max).encode(),str(long_min-1).encode(),str(long_max+1).encode()]
    for loc in locales:
     for value in env_values:
      add('envstoi',value.hex(),locale=loc,initial=34,note='actual swoole_get_env; original std::stoi exception/output/errno versus Rust parse plus native fallback')
    add('envstoi_allocfail',(b'0'*64).hex(),locale='C',initial=34,note='force next C++ allocation to fail; actual >SSO native string construction must throw before Rust or output publication')
    return cases

