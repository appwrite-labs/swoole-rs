"""The HTTP/2 server against a client that writes raw frames, and the server's waits on a peer."""
from pathlib import Path
import hashlib
import json
import socket
import struct
import time

from tools.harness import run_isolated, server
from tests.modules.http2.test_client import PREFACE, frame, read

# :method POST, :path /, :scheme http (RFC 7541 Appendix A), and :authority.
REQUEST = b'\x83\x84\x86\x41\x09localhost'


def test_process_worker_closes_when_ping_ack_exceeds_send_limit(engine, tmp_path):
    script = tmp_path / 'server.php'
    script.write_text("""<?php
$server = new Swoole\\Http\\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_PROCESS);
$server->set(['worker_num' => 1, 'open_http2_protocol' => true, 'output_buffer_size' => 16,
    'log_file' => '/dev/null']);
$server->on('request', static fn ($request, $response) => $response->end());
$server->start();
""")
    with server(engine, script) as (port, _):
        with socket.create_connection(('127.0.0.1', port), timeout=3) as conn:
            conn.sendall(PREFACE + frame(4, 0, 0))
            while True:
                head = read(conn, 9)
                read(conn, int.from_bytes(head[:3], 'big'))
                if head[3:5] == bytes([4, 1]):
                    break
            # A SETTINGS ACK fits. A PING ACK needs 17 bytes.
            conn.sendall(frame(6, 0, 0, b'12345678'))
            assert conn.recv(1) == b''


def test_body_over_the_limit_resets_its_stream(engine):
    """RFC 9113 6.4: a body over package_max_length gets a 4-byte RST_STREAM on
    its own stream, and the connection's framing stays intact."""
    with server(engine, Path(__file__).with_name('body_limit_server.php')) as (port, _):
        with socket.create_connection(('127.0.0.1', port), timeout=10) as conn:
            conn.sendall(PREFACE + frame(4, 0, 0) + frame(1, 4, 1, REQUEST) +
                         frame(0, 0, 1, b'a' * 16000) * 5 + frame(0, 1, 1, b'last'))
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
            statuses = []
            while True:
                head = read(conn, 9)
                payload = read(conn, int.from_bytes(head[:3], 'big'))
                if head[3] == 1 and int.from_bytes(head[5:], 'big') == 1:
                    statuses.append((payload[0], head[4] & 1))
                if head[3] == 6:
                    assert payload == b'afterbad'
                    break
    assert statuses == [(0x8c, 1)]  # HPACK static index 12 is :status 400.


def test_a_peer_that_never_opens_the_window_loses_the_connection(engine):
    """A response waiting on a zero window ends at the send's deadline (the
    server's send_timeout): GOAWAY NO_ERROR, then the connection closes."""
    with server(engine, Path(__file__).with_name('body_limit_server.php')) as (port, _):
        with socket.create_connection(('127.0.0.1', port), timeout=10) as conn:
            # SETTINGS_INITIAL_WINDOW_SIZE 0, and never a WINDOW_UPDATE.
            conn.sendall(PREFACE + frame(4, 0, 0, struct.pack('!HI', 4, 0)) + frame(1, 5, 1, REQUEST))
            began = time.monotonic()
            kinds = []
            while True:
                head = read(conn, 9)
                payload = read(conn, int.from_bytes(head[:3], 'big'))
                kinds.append(head[3])
                if head[3] == 7:
                    break
            waited = time.monotonic() - began
            assert conn.recv(1) == b''
    assert 0 not in kinds, kinds
    assert payload[4:] == struct.pack('!I', 0) + b'flow control or write timeout', payload.hex()
    assert 0.4 < waited < 5, waited


def test_a_command_whose_reply_never_comes_fails_at_the_read_timeout(engine):
    """Server::command() waits for the other process's reply until the default read
    timeout (socket_read_timeout), then fails with SWOOLE_ERROR_CO_TIMEDOUT."""
    code = r'''
    Swoole\Coroutine::set(['socket_read_timeout' => 0.5]);
    $server = new Swoole\Http\Server('127.0.0.1', 0, SWOOLE_PROCESS);
    $server->set(['worker_num' => 1, 'log_file' => '/dev/null']);
    $server->addCommand('stall', SWOOLE_SERVER_COMMAND_EVENT_WORKER, function () {
        sleep(3);
        return 'late';
    });
    $server->on('start', function ($server) {
        $began = microtime(true);
        $result = $server->command('stall', 0, SWOOLE_SERVER_COMMAND_EVENT_WORKER, null, false);
        $timedout = swoole_last_error() === SWOOLE_ERROR_CO_TIMEDOUT;
        echo json_encode([$result, $timedout, microtime(true) - $began]), "\n";
        $server->shutdown();
    });
    $server->on('request', function () {});
    $server->start();
    '''
    result = run_isolated(engine.command('-r', code), timeout=20)
    assert result.returncode == 0, result.stderr
    reply, timedout, waited = json.loads(result.stdout)
    assert (reply, timedout) == (False, True), result.stdout
    assert 0.4 < waited < 2.5, waited


def test_sendfile_delivers_the_selected_range_with_and_without_trailers(engine, tmp_path):
    # Span two complete 64 KiB reads and a partial read, starting inside the file.
    data = bytes(range(251)) * 540
    (tmp_path / 'data.bin').write_bytes(data)
    script = tmp_path / 'sendfile.php'
    script.write_text(r"""<?php
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'open_http2_protocol' => true, 'log_file' => '/dev/null']);
$server->on('request', static function ($request, $response) {
    if ($request->server['request_uri'] === '/trailers') {
        $response->trailer('x-file-end', 'done');
    }
    $response->sendfile(__DIR__ . '/data.bin', 17, 131073);
});
$server->start();
""")
    client = r"""
    Co\run(static function () use ($argv) {
        $client = new Swoole\Coroutine\Http2\Client('127.0.0.1', (int) $argv[1]);
        $client->connect();
        foreach (['/plain', '/trailers'] as $path) {
            $request = new Swoole\Http2\Request();
            $request->path = $path;
            $client->send($request);
            $response = $client->recv(5);
            echo json_encode([$response->statusCode, $response->headers['content-length'],
                hash('sha256', $response->data), $response->headers['x-file-end'] ?? null]), "\n";
        }
        $client->close();
    });
    """
    with server(engine, script) as (port, _):
        result = run_isolated(engine.command('-r', client, '--', str(port)))
    assert result.returncode == 0, result.stderr
    digest = hashlib.sha256(data[17:17 + 131073]).hexdigest()
    assert [json.loads(line) for line in result.stdout.splitlines()] == [
        [200, '131073', digest, None], [200, '131073', digest, 'done']]
