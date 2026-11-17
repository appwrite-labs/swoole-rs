<?php
// Frame::unpack reads its argument: the caller's string, and every copy that
// shares it, is the frame as packed afterwards, so it unpacks again.
use Swoole\WebSocket\Frame;

$failed = [];
foreach ([0, 1, 125, 126, 65536] as $length) {
    $payload = str_repeat('p', $length);
    foreach ([WEBSOCKET_OPCODE_TEXT, WEBSOCKET_OPCODE_BINARY] as $opcode) {
        foreach ([1, 33] as $flags) {
            $packed = Frame::pack($payload, $opcode, $flags);
            $shared = $packed;
            $wire = hash('sha256', $packed);
            foreach ([1, 2] as $pass) {
                $frame = Frame::unpack($packed);
                if (!$frame instanceof Frame || $frame->data !== $payload ||
                    hash('sha256', $packed) !== $wire || hash('sha256', $shared) !== $wire) {
                    $failed[] = "$length/$opcode/$flags/$pass";
                }
            }
        }
    }
}
echo json_encode($failed), "\n";
