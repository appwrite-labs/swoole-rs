<?php
// Private child process, builtin streams only, ephemeral loopback port.
$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $error);
if ($server === false) { throw new RuntimeException("listen: $errno $error"); }
echo stream_socket_get_name($server, false), "\n";
flush();
$socket = stream_socket_accept($server, 10);
if ($socket === false) { throw new RuntimeException('accept timeout'); }
stream_set_timeout($socket, 10);
$received = '';
if (($argv[1] ?? '') === 'receive') {
    while (strlen($received) < 4 && !feof($socket)) {
        $chunk = fread($socket, 4 - strlen($received));
        if ($chunk === false || stream_get_meta_data($socket)['timed_out']) {
            throw new RuntimeException('peer read failed');
        }
        $received .= $chunk;
    }
    if ($received !== 'ping') { throw new RuntimeException('request bytes differ'); }
    if (fwrite($socket, 'pong') !== 4) { throw new RuntimeException('peer write failed'); }
}
while (!feof($socket)) {
    $chunk = fread($socket, 1024);
    if ($chunk === false || stream_get_meta_data($socket)['timed_out']) {
        throw new RuntimeException('peer EOF timeout');
    }
    if ($chunk !== '') { throw new RuntimeException('unexpected extra bytes'); }
}
fclose($socket);
fclose($server);
echo json_encode(['received' => $received, 'eof' => true], JSON_THROW_ON_ERROR), "\n";
