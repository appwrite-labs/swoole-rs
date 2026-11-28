<?php
$mode = $argv[1];
$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $error);
if ($server === false) { throw new RuntimeException("listen: $errno $error"); }
echo stream_socket_get_name($server, false), "\n"; flush();
$requests = [];
for ($i = 0; $i < ($mode === 'retry' ? 2 : 1); $i++) {
    $socket = stream_socket_accept($server, 10);
    if ($socket === false) { throw new RuntimeException('accept timeout'); }
    stream_set_timeout($socket, 10);
    $request = '';
    while (!str_contains($request, "\r\n\r\n")) {
        $bytes = fread($socket, 4096);
        if ($bytes === false || $bytes === '' || strlen($request) > 16384) { throw new RuntimeException('request read failed'); }
        $request .= $bytes;
    }
    $requests[] = explode("\r\n", $request)[0];
    $body = $i === 0 ? 'old' : 'ok';
    $status = $i === 0 ? '502 Bad Gateway' : '200 OK';
    $response = "HTTP/1.1 $status\r\nConnection: close\r\nContent-Length: " . strlen($body)
        . "\r\nSet-Cookie: token=one%20two+three%00tail; Path=/\r\nSet-Cookie: odd;name=value; Path=/\r\nSet-Cookie: nameless\r\n\r\n" . $body;
    while ($response !== '') {
        $written = fwrite($socket, $response);
        if ($written === false || $written === 0) { throw new RuntimeException('response write failed'); }
        $response = substr($response, $written);
    }
    fclose($socket);
}
fclose($server);
echo json_encode(['requests' => $requests], JSON_THROW_ON_ERROR), "\n";
