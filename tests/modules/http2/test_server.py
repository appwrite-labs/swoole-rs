"""The HTTP/2 server against a client that writes raw frames."""
from pathlib import Path
import socket

from tools.harness import server
from tests.modules.http2.test_client import PREFACE, frame, read

# :method POST, :path /, :scheme http (RFC 7541 Appendix A), and :authority.
REQUEST = b'\x83\x84\x86\x41\x09localhost'


def test_body_over_the_limit_resets_its_stream(engine):
    """RFC 9113 6.4: a body over package_max_length gets a 4-byte RST_STREAM on
    its own stream, and the connection's framing stays intact."""
    with server(engine, Path(__file__).with_name('body_limit_server.php')) as (port, _):
        with socket.create_connection(('127.0.0.1', port), timeout=10) as conn:
            conn.sendall(PREFACE + frame(4, 0, 0) + frame(1, 4, 1, REQUEST) +
                         frame(0, 0, 1, b'a' * 16000) * 5)
            while True:
                head = read(conn, 9)
                payload = read(conn, int.from_bytes(head[:3], 'big'))
                if head[3] == 3:
                    break
            assert (int.from_bytes(head[5:], 'big'), payload) == (1, bytes(4)), (head + payload).hex()
            # What follows the reset is still a frame: here a PING ACK.
            conn.sendall(frame(6, 0, 0, b'stillok!'))
            while True:
                after = read(conn, 9)
                echo = read(conn, int.from_bytes(after[:3], 'big'))
                if after[3] == 6:
                    break
    assert (after[4], echo) == (1, b'stillok!'), (after + echo).hex()


def test_a_body_the_multipart_parser_rejects_is_never_handled(engine):
    """A multipart body that fails to parse never reaches onRequest; its stream's
    objects are released all the same (the ASan lane checks for leaks)."""
    # content-type, static index 31, as a literal without indexing (RFC 7541 6.2.2).
    content_type = b'multipart/form-data; boundary=x'
    block = REQUEST + b'\x0f\x10' + bytes([len(content_type)]) + content_type
    body = b'--x\r\nno colon here\r\n\r\nv\r\n--x--\r\n'
    with server(engine, Path(__file__).with_name('body_limit_server.php')) as (port, _):
        with socket.create_connection(('127.0.0.1', port), timeout=10) as conn:
            conn.sendall(PREFACE + frame(4, 0, 0) + frame(1, 4, 1, block) + frame(0, 1, 1, body))
            # Then a PING, so the server's reply to it bounds the wait.
            conn.sendall(frame(6, 0, 0, b'afterbad'))
            kinds = []
            try:
                while True:
                    head = read(conn, 9)
                    read(conn, int.from_bytes(head[:3], 'big'))
                    kinds.append((head[3], int.from_bytes(head[5:], 'big') & 0x7fffffff))
                    if head[3] == 6:
                        break
            except (EOFError, ConnectionResetError, socket.timeout):
                pass
    assert (1, 1) not in kinds, kinds
