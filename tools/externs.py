#!/usr/bin/env python3
"""Check hand-written `extern "C"` declarations under crates/ against the C headers.

A `safe fn` in an extern block is called with no `unsafe`, so a wrong arity or
argument width there is undefined behaviour that nothing else catches. For each
declared function the headers below also declare, this writes a C++ file that
reduces the C prototype to a shape string and static_asserts that it equals the
Rust declaration's: return type first, then each parameter as `p` (pointer),
`1`/`2`/`4`/`8` (integer or enum of that many bytes), `f`/`d` (float, double),
`a` (struct by value) or `v` (void), and `.` if variadic. Signedness and pointee
types are not compared.

Rust types reduce to shapes by name: references, raw and function pointers,
`NonNull` and `Box` are pointers, `Option<T>` is `T`, a `#[repr(transparent)]`
struct is its first non-PhantomData field, a type alias its target, and the
scalars come from SCALARS. A name found in neither fails the check: add it.

The headers are PHP's (`php-config --includes`), zend-sys's shims, OpenSSL's and
glibc's. A declaration none of them declares (another library's, a Rust export,
a C shim with no header) is unchecked; `--unchecked` lists those.
"""
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SCALARS = {
    'i8': '1', 'u8': '1', 'bool': '1', 'c_char': '1', 'c_schar': '1', 'c_uchar': '1',
    'i16': '2', 'u16': '2', 'c_short': '2', 'c_ushort': '2',
    'i32': '4', 'u32': '4', 'c_int': '4', 'c_uint': '4', 'RawFd': '4', 'OwnedFd': '4',
    'i64': '8', 'u64': '8', 'c_long': '8', 'c_ulong': '8', 'c_longlong': '8', 'c_ulonglong': '8',
    'usize': '8', 'isize': '8', 'f32': 'f', 'f64': 'd', 'c_float': 'f', 'c_double': 'd', '()': 'v', '!': 'v',
    # libc's, on Linux.
    'int32_t': '4', 'uint32_t': '4', 'pid_t': '4', 'uid_t': '4', 'gid_t': '4', 'id_t': '4', 'mode_t': '4',
    'socklen_t': '4', 'key_t': '4', 'useconds_t': '4', 'clockid_t': '4', '__priority_which_t': '4',
    '__rlimit_resource_t': '4', 'int64_t': '8', 'uint64_t': '8', 'size_t': '8', 'ssize_t': '8', 'off_t': '8',
    'time_t': '8', 'nfds_t': '8', 'intptr_t': '8', 'uintptr_t': '8', 'pthread_t': '8',
    # bindgen's, from zend-sys's generated bindings.rs.
    'zend_uchar': '1', 'zend_bool': '1', 'zend_result': '4', 'zend_fiber_status': '4', 'zend_long': '8',
    'zend_ulong': '8', 'swr_zend_then': 'p', 'zif_handler': 'p',
    # A struct iovec.
    'IoSlice': 'a', 'IoSliceMut': 'a',
}
POINTERS = {'NonNull', 'Box', 'c_void'}
WRAPPERS = {'Option', 'Cell', 'UnsafeCell', 'ManuallyDrop', 'MaybeUninit'}
# Included before zend-sys's shim headers.
HEADERS = [
    'php_variables.h', 'ext/pcre/php_pcre.h', 'ext/standard/php_var.h', 'ext/pdo/php_pdo_driver.h',
    'openssl/ssl.h', 'openssl/err.h', 'openssl/x509v3.h', 'openssl/rand.h',
    'arpa/inet.h', 'dlfcn.h', 'execinfo.h', 'fcntl.h', 'grp.h', 'ifaddrs.h', 'locale.h', 'malloc.h', 'net/if.h',
    'netdb.h', 'poll.h', 'pthread.h', 'pwd.h', 'sched.h', 'signal.h', 'spawn.h', 'syslog.h', 'termios.h',
    'time.h', 'unistd.h', 'sys/epoll.h', 'sys/eventfd.h', 'sys/file.h', 'sys/inotify.h', 'sys/ipc.h',
    'sys/mman.h', 'sys/msg.h', 'sys/prctl.h', 'sys/random.h', 'sys/resource.h', 'sys/sem.h', 'sys/sendfile.h',
    'sys/shm.h', 'sys/signalfd.h', 'sys/socket.h', 'sys/timerfd.h', 'sys/utsname.h', 'sys/wait.h',
]

PREAMBLE = r'''
#include <type_traits>
template <class T> constexpr char shape() {
    if constexpr (std::is_void_v<T>) return 'v';
    else if constexpr (std::is_pointer_v<T>) return 'p';
    else if constexpr (std::is_floating_point_v<T>) return sizeof(T) == 4 ? 'f' : 'd';
    else if constexpr (std::is_integral_v<T> || std::is_enum_v<T>) return char('0' + sizeof(T));
    else return 'a';
}
template <class R, class... A> struct Shape { static constexpr char s[] = {shape<R>(), shape<A>()..., 0}; };
template <class R, class... A> struct Variadic { static constexpr char s[] = {shape<R>(), shape<A>()..., '.', 0}; };
template <class R, class... A> constexpr const char *of(R (*)(A...)) { return Shape<R, A...>::s; }
template <class R, class... A> constexpr const char *of(R (*)(A..., ...)) { return Variadic<R, A...>::s; }
constexpr bool same(const char *a, const char *b) { return *a == *b && (!*a || same(a + 1, b + 1)); }
'''


def strip(text):
    text = re.sub(r'/\*.*?\*/', ' ', text, flags=re.S)
    return re.sub(r'//[^\n]*', ' ', text)


def split(text, sep=','):
    depth, current, out = 0, '', []
    for ch in text:
        if ch == sep and depth == 0:
            out.append(current)
            current = ''
            continue
        depth += ch in '<(['
        depth -= ch in '>)]' and not current.endswith('-')
        current += ch
    if current.strip():
        out.append(current)
    return [p.strip() for p in out]


def block(text, start):
    depth, i = 1, start
    while depth:
        depth += text[i] == '{'
        depth -= text[i] == '}'
        i += 1
    return text[start:i - 1]


def enabled(attributes):
    """Whether `#[cfg(...)]` in `attributes` holds on Linux with every feature on."""
    def holds(pred):
        pred = pred.strip()
        op = re.match(r'(all|any|not)\s*\((.*)\)$', pred, re.S)
        if op:
            parts = [holds(p) for p in split(op.group(2))]
            return {'all': all, 'any': any}.get(op.group(1), lambda p: not p[0])(parts)
        key = re.match(r'(\w+)\s*=\s*"(\w+)"', pred)
        if key and key.group(1) in ('target_os', 'target_vendor', 'target_env'):
            return key.group(2) in ('linux', 'unknown', 'gnu')
        return pred not in ('windows', 'test')
    return all(holds(c) for c in re.findall(r'#\[cfg\((.*?)\)\]\s*(?=#|$)', attributes.strip() + ' ', re.S))


def declarations(path, text):
    """(where, symbol, [param types], return type, variadic) of each fn in an extern "C" block."""
    out = []
    text = strip(text)
    for match in re.finditer(r'extern\s+"C"\s*\{', text):
        if not enabled(re.search(r'((?:#\[[^\]]*\]\s*)*)(?:unsafe\s+)?$', text[:match.start()]).group(1)):
            continue
        line = text.count('\n', 0, match.start()) + 1
        body = block(text, match.end())
        for item in re.finditer(r'((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?(?:safe\s+|unsafe\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(', body):
            if not enabled(item.group(1)):
                continue
            link = re.search(r'#\[link_name\s*=\s*"([^"]+)"', item.group(1))
            symbol = link.group(1) if link else item.group(2)
            for pred, alias in re.findall(r'cfg_attr\(([^\]]*?),\s*link_name\s*=\s*"([^"]+)"\s*\)\]', item.group(1)):
                symbol = alias if enabled(f'#[cfg({pred})]') else symbol
            depth, i = 1, item.end()
            while depth:
                depth += body[i] == '('
                depth -= body[i] == ')'
                i += 1
            params = split(body[item.end():i - 1])
            ret = re.match(r'\s*(?:->\s*([^;{]+?))?\s*[;{]', body[i:])
            variadic = bool(params) and params[-1] == '...'
            params = [p.split(':', 1)[1].strip() for p in params if p != '...']
            where = f'{path}:{line + body.count(chr(10), 0, item.start(2))}'
            out.append((where, symbol, params, (ret and ret.group(1)) or '()', variadic))
    return out


def wrappers(sources):
    """{name: {crate: (generic parameters, what it is)}} of each #[repr(transparent)] struct's
    first field, #[repr(C)] struct, #[repr] enum, type alias and `use ... as` rename."""
    out = {}

    def add(crate, name, generics, field):
        out.setdefault(name, {}).setdefault(crate, (generics, field))

    for path, text in sources.items():
        crate = path.relative_to(ROOT / 'crates').parts[0].replace('-', '_')
        text = strip(text)
        for m in re.finditer(r'#\[repr\((transparent|C)\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)\s*(?:<([^{(;]*?)>)?\s*([({])', text):
            generics = [g.split(':')[0].split('=') for g in split(m.group(3) or '')]
            if m.group(1) == 'C':
                add(crate, m.group(2), [], '[struct]')  # an aggregate, as an array is
                continue
            fields = block(text, m.end()) if m.group(4) == '{' else text[m.end():text.index(';', m.end())]
            fields = [re.sub(r'^(?:pub(?:\([^)]*\))?\s+)?(?:\w+\s*:(?!:))?', '', f.strip()).strip()
                      for f in split(fields.rstrip().rstrip(')'))]
            fields = [f for f in fields if f and not re.match(r'(\w+::)*PhantomData\b', f)]
            if fields:
                add(crate, m.group(2), [[x.strip() for x in g] for g in generics], fields[0])
        for m in re.finditer(r'#\[repr\((u8|i8|u16|i16|u32|i32|u64|i64|C|usize|isize)\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?enum\s+(\w+)', text):
            add(crate, m.group(2), [], 'i32' if m.group(1) == 'C' else m.group(1))
        for m in re.finditer(r'\btype\s+(\w+)\s*(?:<[^=]*>)?\s*=\s*([^;]+);', text):
            add(crate, m.group(1), [], m.group(2).strip())
        for m in re.finditer(r'\b(\w+)\s+as\s+(\w+)\s*[,;}]', text):
            if m.group(1) != m.group(2) and not m.group(1)[0].islower():
                add(crate, m.group(2), [], m.group(1))
    return out


def rust_shape(ty, known, crates, depth=0):
    """The shape of Rust type `ty`, looking names up in `crates` first."""
    ty = re.sub(r"'\w+\s*,?\s*", '', re.sub(r'^for<[^>]*>', '', ty.strip())).replace('<>', '').strip()
    if ty.startswith('['):
        return 'a'
    if ty.startswith(('&', '*')) or re.match(r'(unsafe\s+)?(extern\s+"C"\s+)?fn\b', ty):
        return 'p'
    named = re.match(r'(?:\w+::)*?swoole_rs_(\w+)::', ty)
    if named:
        crates = [named.group(1), *crates]
    ty = re.sub(r'^(?:dyn\s+)?(?:\w+::)*', '', ty)
    name, _, args = ty.partition('<')
    args = split(args[:-1]) if args else []
    if ty in SCALARS:
        return SCALARS[ty]
    if name in POINTERS:
        return 'p'
    if name in WRAPPERS and args:
        return rust_shape(args[0], known, crates, depth + 1)
    defs = known.get(name, {})
    crate = next((c for c in crates if c in defs), None) or next(iter(sorted(defs)), None)
    if crate is None or depth > 20:
        return None
    generics, field = defs[crate]
    generics = [g for g in generics if not g[0].startswith("'")]
    for i, generic in enumerate(generics):
        arg = args[i] if i < len(args) else (generic[1] if len(generic) > 1 else None)
        if arg:
            field = re.sub(r'\b' + re.escape(generic[0]) + r'\b', arg, field)
    return rust_shape(field, known, [crate, *crates], depth + 1)


def generate(decls, known, declared):
    """The C++ source checking `decls` that the headers declare, and the types it can't map."""
    lines, unknown = [], []
    for where, symbol, params, ret, variadic, crates in decls:
        if symbol not in declared:
            continue
        shapes = [rust_shape(t, known, crates) for t in [ret, *params]]
        if None in shapes:
            unknown.append(f'{where}: {symbol}: no shape for ' + ', '.join(t for t, s in zip([ret, *params], shapes) if s is None))
            continue
        expect = ''.join(shapes) + ('.' if variadic else '')
        lines.append(f'static_assert(same(of(&{symbol}), "{expect}"), "{where}: {symbol}: the C prototype is not {expect}");')
    return '\n'.join(lines), unknown


def check(sources, headers, flags):
    """Problems with the declarations in `sources` ({path: Rust text}) that `headers` (C
    `#include` lines, compiled with `flags`) declare, and the declarations left unchecked."""
    compiler = ['g++', '-std=c++17', '-x', 'c++', *flags]
    expanded = subprocess.run([*compiler, '-E', '-'], input=headers, text=True, capture_output=True, check=True).stdout
    decls = []
    for path, text in sources.items():
        # Names resolve in the declaring crate first, then in the crates its file uses.
        crates = [path.relative_to(ROOT / 'crates').parts[0].replace('-', '_'),
                  *dict.fromkeys(re.findall(r'\bswoole_rs_(\w+)', text))]
        decls += [(*d, crates) for d in declarations(path.relative_to(ROOT), text)]
    names = {d[1] for d in decls}
    declared = {m.group(1) for m in re.finditer(r'\b(\w+)\s*\(', expanded) if m.group(1) in names}
    source, problems = generate(decls, wrappers(sources), declared)
    with tempfile.NamedTemporaryFile('w', suffix='.cpp') as tmp:
        tmp.write(headers + PREAMBLE + source + '\n')
        tmp.flush()
        result = subprocess.run([*compiler, '-fsyntax-only', '-fmax-errors=0', tmp.name], text=True, capture_output=True)
    # Clang (g++ on macOS) quotes the failed expression first: "failed due to requirement '...': ".
    errors = re.findall(r"error: static assertion failed(?: due to requirement '[^']*')?: (.*)", result.stderr)
    problems += errors or ([result.stderr] if result.returncode else [])
    return problems, [f'{d[0]}: {d[1]}' for d in decls if d[1] not in declared]


def main():
    includes = subprocess.check_output(['php-config', '--includes'], text=True).split()
    shims = ROOT / 'crates/zend-sys/src'
    headers = ''.join(f'#include "{h}"\n' for h in [*HEADERS, *(h.name for h in sorted(shims.glob('*.h')))])
    sources = {p: p.read_text() for p in sorted((ROOT / 'crates').glob('*/src/**/*.rs'))
               if 'oracle/upstream' not in str(p)}
    problems, unchecked = check(sources, headers, [*includes, f'-I{shims}'])
    if '--unchecked' in sys.argv:
        print('\n'.join(unchecked))
    for problem in problems:
        print(problem, file=sys.stderr)
    total = sum(len(declarations(p, t)) for p, t in sources.items())
    print(f'externs: {total - len(unchecked)} of {total} extern "C" functions checked; '
          f'the rest are in no header here (--unchecked lists them)')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
