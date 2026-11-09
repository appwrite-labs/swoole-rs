"""HTTP/1 framing ambiguities sent to a whole server on the wire, compared with the control engine.

Each case is one byte stream on a fresh connection. The outcome is every response the
server sent (status line, Connection header and body; the handler's body says what it
received) and whether the server closed the connection.
"""
from concurrent.futures import ThreadPoolExecutor
import functools
import json
from pathlib import Path
import socket

import pytest

from tools.harness import server

IDLE = 1.0  # seconds without a byte before a still-open connection's outcome is final


def req(head=b'GET / HTTP/1.1', headers=(), body=b'', eol=b'\r\n'):
    return eol.join([head, b'Host: localhost', *headers, b'', b'']) + body


def post(headers, body):
    return req(b'POST / HTTP/1.1', headers, body)


def chunked(body, te=b'chunked', extra=()):
    return post([b'Transfer-Encoding: ' + te, *extra], body)


def cases():
    rows = {
        'get': req(),
        'get-http10': req(b'GET / HTTP/1.0'),
        'get-query': req(b'GET /a/b?x=1&y=2 HTTP/1.1'),
        'post-length': post([b'Content-Length: 5'], b'hello'),
        'post-chunked': chunked(b'5\r\nhello\r\n0\r\n\r\n'),
        'pipelined-two': req() + req(b'GET /second HTTP/1.1'),
        'pipelined-three': req() + post([b'Content-Length: 1'], b'x') + req(b'GET /third HTTP/1.1'),
        'connection-close': req(headers=[b'Connection: close']) + req(b'GET /after HTTP/1.1'),
    }
    # Content-Length.
    for name, values in {
        'cl-duplicate-same': [b'5', b'5'], 'cl-duplicate-differ': [b'5', b'6'],
        'cl-duplicate-differ-reverse': [b'6', b'5'], 'cl-list-same': [b'5, 5'], 'cl-list-differ': [b'5, 6'],
        'cl-plus': [b'+5'], 'cl-leading-zeros': [b'005'], 'cl-negative': [b'-1'], 'cl-hex': [b'0x5'],
        'cl-trailing-space': [b'5 '], 'cl-leading-tab': [b'\t5'], 'cl-empty': [b''], 'cl-suffix': [b'5a'],
        'cl-decimal': [b'5.0'], 'cl-overflow': [b'18446744073709551621'], 'cl-huge': [b'99999999999999999999999'],
        'cl-zero': [b'0'], 'cl-short': [b'3'], 'cl-over-max': [b'70000'],
    }.items():
        rows[name] = post([b'Content-Length: ' + value for value in values], b'hello')
    rows['cl-name-lowercase'] = post([b'content-length: 5'], b'hello')
    rows['cl-name-space-before-colon'] = post([b'Content-Length : 5'], b'hello')
    rows['cl-no-space'] = post([b'Content-Length:5'], b'hello')
    rows['cl-obs-fold'] = post([b'Content-Length:', b' 5'], b'hello')
    rows['cl-on-get'] = req(headers=[b'Content-Length: 5'], body=b'hello')
    rows['cl-zero-then-request'] = post([b'Content-Length: 0'], req(b'GET /smuggled HTTP/1.1'))
    rows['cl-body-is-request'] = post([b'Content-Length: %d' % len(req(b'GET /smuggled HTTP/1.1'))], req(b'GET /smuggled HTTP/1.1'))
    # Transfer-Encoding.
    body = b'5\r\nhello\r\n0\r\n\r\n'
    for name, te in {
        'te-capitalised': b'Chunked', 'te-upper': b'CHUNKED', 'te-leading-space': b'  chunked',
        'te-trailing-space': b'chunked ', 'te-leading-tab': b'\tchunked', 'te-vertical-tab': b'\x0bchunked',
        'te-form-feed': b'\x0cchunked', 'te-identity-chunked': b'identity, chunked', 'te-gzip-chunked': b'gzip, chunked',
        'te-chunked-gzip': b'chunked, gzip', 'te-chunked-identity': b'chunked, identity', 'te-chunked-chunked': b'chunked, chunked',
        'te-prefixed': b'xchunked', 'te-suffixed': b'chunkedx', 'te-quoted': b'"chunked"', 'te-parameter': b'chunked;q=1',
        'te-no-comma': b'gzip chunked', 'te-identity': b'identity', 'te-unknown': b'foo', 'te-empty': b'',
    }.items():
        rows[name] = chunked(body, te)
    rows['te-duplicate'] = post([b'Transfer-Encoding: chunked', b'Transfer-Encoding: identity'], body)
    rows['te-duplicate-reverse'] = post([b'Transfer-Encoding: identity', b'Transfer-Encoding: chunked'], body)
    rows['te-name-space-before-colon'] = post([b'Transfer-Encoding : chunked'], body)
    rows['te-name-underscore'] = post([b'Transfer_Encoding: chunked'], body)
    rows['te-obs-fold'] = post([b'Transfer-Encoding:', b' chunked'], body)
    rows['te-obs-fold-after'] = post([b'Transfer-Encoding: gzip', b' , chunked'], body)
    rows['te-http10'] = req(b'POST / HTTP/1.0', [b'Transfer-Encoding: chunked'], body)
    rows['te-on-get'] = req(headers=[b'Transfer-Encoding: chunked'], body=body)
    # Both framings: the classic CL.TE and TE.CL desyncs, and obfuscated TE.TE.
    smuggled = req(b'GET /smuggled HTTP/1.1')
    rows['cl-te'] = post([b'Content-Length: 6', b'Transfer-Encoding: chunked'], b'0\r\n\r\nG' + smuggled)
    rows['te-cl'] = post([b'Transfer-Encoding: chunked', b'Content-Length: 6'], b'0\r\n\r\nG' + smuggled)
    rows['cl-te-zero-chunk-smuggle'] = post([b'Content-Length: 5', b'Transfer-Encoding: chunked'], b'0\r\n\r\n' + smuggled)
    rows['te-cl-long-chunk'] = post([b'Content-Length: 4', b'Transfer-Encoding: chunked'],
                                    b'%x\r\n' % (len(smuggled) + 2) + b'X:' + smuggled + b'\r\n0\r\n\r\n')
    rows['te-te-obfuscated'] = post([b'Transfer-Encoding: chunked', b'Transfer-Encoding: x'], b'0\r\n\r\n' + smuggled)
    rows['te-cl-obfuscated-te'] = post([b'Content-Length: 4', b'Transfer-Encoding: xchunked'], b'%x\r\n' % len(smuggled) + smuggled + b'\r\n0\r\n\r\n')
    rows['cl-te-capitalised'] = post([b'Content-Length: 6', b'Transfer-Encoding: Chunked'], b'0\r\n\r\nG' + smuggled)
    rows['cl-te-space-colon'] = post([b'Content-Length: 6', b'Transfer-Encoding : chunked'], b'0\r\n\r\nG' + smuggled)
    # Line endings.
    rows['lf-only'] = req(eol=b'\n')
    rows['lf-only-post'] = req(b'POST / HTTP/1.1', [b'Content-Length: 5'], b'hello', eol=b'\n')
    rows['lf-request-line'] = b'GET / HTTP/1.1\nHost: localhost\r\n\r\n'
    rows['lf-header-line'] = b'GET / HTTP/1.1\r\nHost: localhost\nX-A: 1\r\n\r\n'
    rows['lf-end-of-headers'] = b'GET / HTTP/1.1\r\nHost: localhost\r\n\n'
    rows['cr-only'] = req(eol=b'\r')
    rows['cr-cr-lf'] = req(eol=b'\r\r\n')
    rows['cr-in-header-value'] = req(headers=[b'X-A: a\rb'])
    rows['lf-in-header-value'] = req(headers=[b'X-A: a\nX-Smuggled: b'])
    rows['lf-before-cl'] = b'POST / HTTP/1.1\r\nHost: localhost\nContent-Length: 5\r\n\r\nhello'
    rows['lf-chunk-size'] = chunked(b'5\nhello\r\n0\r\n\r\n')
    rows['lf-chunk-data'] = chunked(b'5\r\nhello\n0\r\n\r\n')
    rows['lf-last-chunk'] = chunked(b'5\r\nhello\r\n0\n\n')
    rows['cr-chunk-size'] = chunked(b'5\rhello\r\n0\r\n\r\n')
    # Chunk sizes, extensions and trailers.
    for name, data in {
        'chunk-hex-upper': b'A\r\nhellohello\r\n0\r\n\r\n', 'chunk-hex-lower': b'a\r\nhellohello\r\n0\r\n\r\n',
        'chunk-leading-zeros': b'0005\r\nhello\r\n0\r\n\r\n', 'chunk-0x': b'0x5\r\nhello\r\n0\r\n\r\n',
        'chunk-negative': b'-5\r\nhello\r\n0\r\n\r\n', 'chunk-plus': b'+5\r\nhello\r\n0\r\n\r\n',
        'chunk-leading-space': b' 5\r\nhello\r\n0\r\n\r\n', 'chunk-trailing-space': b'5 \r\nhello\r\n0\r\n\r\n',
        'chunk-trailing-tab': b'5\t\r\nhello\r\n0\r\n\r\n', 'chunk-invalid-hex': b'g\r\nhello\r\n0\r\n\r\n',
        'chunk-empty-size': b'\r\nhello\r\n0\r\n\r\n', 'chunk-overflow': b'10000000000000005\r\nhello\r\n0\r\n\r\n',
        'chunk-huge': b'ffffffffffffffffffff\r\nhello\r\n0\r\n\r\n', 'chunk-over-max': b'11171\r\nhello\r\n0\r\n\r\n',
        'chunk-extension': b'5;foo=bar\r\nhello\r\n0\r\n\r\n', 'chunk-extension-quoted': b'5;foo="b;a\\"r"\r\nhello\r\n0\r\n\r\n',
        'chunk-extension-empty': b'5;\r\nhello\r\n0\r\n\r\n', 'chunk-extension-space': b'5 ;foo=bar\r\nhello\r\n0\r\n\r\n',
        'chunk-extension-lf': b'5;foo\nbar\r\nhello\r\n0\r\n\r\n', 'chunk-extension-on-last': b'5\r\nhello\r\n0;foo=bar\r\n\r\n',
        'chunk-data-long': b'3\r\nhello\r\n0\r\n\r\n', 'chunk-data-short': b'7\r\nhello\r\n0\r\n\r\n',
        'chunk-missing-crlf': b'5\r\nhello0\r\n\r\n', 'chunk-missing-last': b'5\r\nhello\r\n',
        'chunk-missing-final-crlf': b'5\r\nhello\r\n0\r\n', 'chunk-last-zeros': b'5\r\nhello\r\n000\r\n\r\n',
        'chunk-many': b''.join(b'1\r\n%c\r\n' % c for c in b'abcdefghij') + b'0\r\n\r\n', 'chunk-zero-only': b'0\r\n\r\n',
        'trailer': b'5\r\nhello\r\n0\r\nX-Trailer: 1\r\n\r\n',
        'trailer-content-length': b'5\r\nhello\r\n0\r\nContent-Length: 50\r\n\r\n',
        'trailer-transfer-encoding': b'5\r\nhello\r\n0\r\nTransfer-Encoding: chunked\r\n\r\n',
        'trailer-no-colon': b'5\r\nhello\r\n0\r\nbroken\r\n\r\n',
        'trailer-then-request': b'5\r\nhello\r\n0\r\nX-Trailer: 1\r\n\r\n' + smuggled,
    }.items():
        rows[name] = chunked(data)
    rows['chunked-then-request'] = chunked(body + smuggled)
    # Oversized and malformed headers and request lines.
    rows['header-8k'] = req(headers=[b'X-Long: ' + b'a' * 8192])
    rows['header-60k'] = req(headers=[b'X-Long: ' + b'a' * 60000])
    rows['header-70k'] = req(headers=[b'X-Long: ' + b'a' * 70000])
    rows['headers-many'] = req(headers=[b'X-%d: %d' % (i, i) for i in range(500)])
    rows['header-no-colon'] = req(headers=[b'X-Broken'])
    rows['header-name-space'] = req(headers=[b'X Broken: 1'])
    rows['header-name-empty'] = req(headers=[b': 1'])
    rows['header-name-at'] = req(headers=[b'X@A: 1'])
    rows['header-name-nul'] = req(headers=[b'X\x00A: 1'])
    rows['header-value-nul'] = req(headers=[b'X-A: a\x00b'])
    rows['header-value-del'] = req(headers=[b'X-A: a\x7fb'])
    rows['header-value-high'] = req(headers=[b'X-A: \xff\xfe'])
    rows['header-obs-fold'] = req(headers=[b'X-A: first', b' second'])
    rows['header-leading-space'] = b'GET / HTTP/1.1\r\n Host: localhost\r\n\r\n'
    rows['header-duplicate'] = req(headers=[b'X-A: 1', b'X-A: 2'])
    rows['host-duplicate'] = req(headers=[b'Host: other'])
    rows['host-missing'] = b'GET / HTTP/1.1\r\n\r\n'
    rows['method-lowercase'] = req(b'get / HTTP/1.1')
    rows['method-unknown'] = req(b'FOO / HTTP/1.1')
    rows['method-with-body-unknown'] = req(b'FOO / HTTP/1.1', [b'Content-Length: 5'], b'hello')
    rows['request-line-double-space'] = req(b'GET  / HTTP/1.1')
    rows['request-line-trailing-space'] = req(b'GET / HTTP/1.1 ')
    rows['request-line-tab'] = req(b'GET\t/\tHTTP/1.1')
    rows['request-line-no-version'] = req(b'GET /')
    rows['request-line-leading-crlf'] = b'\r\n' + req()
    rows['version-11-lower'] = req(b'GET / http/1.1')
    rows['version-12'] = req(b'GET / HTTP/1.2')
    rows['version-20'] = req(b'GET / HTTP/2.0')
    rows['version-09'] = b'GET /\r\n'
    rows['uri-absolute'] = req(b'GET http://other/path HTTP/1.1')
    rows['uri-asterisk'] = req(b'OPTIONS * HTTP/1.1')
    rows['uri-space'] = req(b'GET /a b HTTP/1.1')
    rows['uri-fragment'] = req(b'GET /a#frag HTTP/1.1')
    rows['uri-long'] = req(b'GET /' + b'a' * 16384 + b' HTTP/1.1')
    rows['uri-nul'] = req(b'GET /a\x00b HTTP/1.1')
    rows['garbage'] = b'\x00\x01\x02 not http\r\n\r\n'
    # Requests pipelined behind an error.
    rows['valid-then-invalid'] = req() + b'BROKEN\r\n\r\n'
    rows['invalid-then-valid'] = b'BROKEN\r\n\r\n' + req()
    rows['bad-cl-then-valid'] = post([b'Content-Length: x'], b'') + req(b'GET /after HTTP/1.1')
    rows['bad-chunk-then-valid'] = chunked(b'g\r\n\r\n') + req(b'GET /after HTTP/1.1')
    rows['oversized-then-valid'] = req(headers=[b'X-Long: ' + b'a' * 70000]) + req(b'GET /after HTTP/1.1')
    rows['over-max-cl-then-valid'] = post([b'Content-Length: 70000'], b'') + req(b'GET /after HTTP/1.1')
    rows['lf-then-valid'] = req(eol=b'\n') + req(b'GET /after HTTP/1.1')
    rows['http10-then-valid'] = req(b'GET / HTTP/1.0') + req(b'GET /after HTTP/1.1')
    return rows


CASES = cases()


def responses(data):
    found = []
    while data:
        head, sep, data = data.partition(b'\r\n\r\n')
        if not sep:
            found.append({'incomplete': head.decode('latin-1')})
            break
        status, *lines = head.decode('latin-1').split('\r\n')
        fields = dict((name.strip().lower(), value.strip()) for name, _, value in (line.partition(':') for line in lines))
        if 'content-length' in fields:
            size = int(fields['content-length'])
            body, data = data[:size], data[size:]
        else:
            body, data = data, b''
        try:
            body = json.loads(body)
        except ValueError:
            body = body.decode('latin-1')
        found.append({'status': status, 'connection': fields.get('connection'), 'body': body})
    return found


def exchange(port, data):
    received, closed = b'', False
    with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
        try:
            conn.sendall(data)
        except OSError:
            pass  # the server may close before reading everything; what it sent still counts
        conn.settimeout(IDLE)
        while True:
            try:
                chunk = conn.recv(65536)
            except TimeoutError:
                break
            except ConnectionResetError:
                closed = True
                break
            if not chunk:
                closed = True
                break
            received += chunk
    return {'responses': responses(received), 'closed': closed}


@functools.cache
def outcomes(engine):
    with server(engine, Path(__file__).with_name('smuggling.php')) as (port, _):
        with ThreadPoolExecutor(32) as pool:
            found = dict(zip(CASES, pool.map(functools.partial(exchange, port), CASES.values())))
        found['health-after'] = exchange(port, req(headers=[b'Connection: close']))
    return found


def test_case_count():
    assert len(CASES) >= 100


def test_engine_serves(engine):
    """Equal outcomes mean nothing if the server answered nothing at all."""
    for name in ['get', 'post-length', 'post-chunked', 'health-after']:
        reply = outcomes(engine)[name]['responses']
        assert reply and reply[0]['status'] == 'HTTP/1.1 200 OK', (name, reply)


@pytest.mark.parametrize('name', [*CASES, 'health-after'])
def test_framing(engine, control, name):
    assert outcomes(engine)[name] == outcomes(control)[name]
