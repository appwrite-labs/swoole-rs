<?php
// Protocol testee only: the independent Autobahn driver owns all assertions.
$server = new Swoole\WebSocket\Server('0.0.0.0', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'websocket_compression' => true,
    'package_max_length' => 64 * 1024 * 1024, 'buffer_output_size' => 64 * 1024 * 1024,
    'log_file' => '/dev/null']);
$server->on('message', static function ($server, $frame): void {
    // Preserve text/binary opcode; Swoole reassembles fragments before this callback.
    if (!$server->push($frame->fd, $frame->data, $frame->opcode)) {
        throw new RuntimeException('Autobahn echo failed');
    }
});
$server->start();
