<?php
$mode = $argv[1];
$server = stream_socket_server('udp://127.0.0.1:0', $errno, $error, STREAM_SERVER_BIND);
if ($server === false) { throw new RuntimeException("listen: $errno $error"); }
echo stream_socket_get_name($server, false), "\n"; flush();
stream_set_timeout($server, 10);
$query = stream_socket_recvfrom($server, 4096, 0, $sender);
if ($query === false || strlen($query) < 18) { throw new RuntimeException('query missing'); }
$expectedName = "\5probe\5codec\4test\0";
if (substr($query,12,18) !== $expectedName) { throw new RuntimeException('exact encoded name differs'); }
$type = $mode === 'aaaa' ? 28 : 1;
if (substr($query,30) !== pack('nn', $type, 1)) { throw new RuntimeException('exact query type/class/length differs'); }
$header = unpack('nid/nflags/nquestions/nanswers/nauthority/nadditional', substr($query,0,12));
if ($header['questions'] !== 1 || $header['answers'] !== 0) { throw new RuntimeException('query counts differ'); }
$address = $mode === 'aaaa' ? inet_pton('2001:db8::42') : inet_pton('192.0.2.42');
$id = $mode === 'bad-id' ? ($header['id'] + 1) % 65536 : $header['id'];
$response = pack('nnnnnn', $id, 0x8180, 1, 1, 0, 0) . substr($query,12)
    . "\xc0\x0c" . pack('nnNn', $type, 1, 30, strlen($address)) . $address;
if (stream_socket_sendto($server,$response,0,$sender) !== strlen($response)) { throw new RuntimeException('response send failed'); }
fclose($server);
echo json_encode(['name_hex'=>bin2hex(substr($query,12,18)), 'type'=>$type,
    'id_matches'=>$id===$header['id'], 'response_bytes'=>strlen($response)],JSON_THROW_ON_ERROR), "\n";
