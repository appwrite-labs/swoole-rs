<?php
declare(strict_types=1);

use Swoole\WebSocket\Frame;

try {
    [$script, $count, $size, $masked] = $argv;
    $iterations = filter_var($count, FILTER_VALIDATE_INT, ['options' => ['min_range' => 1]]);
    $length = filter_var($size, FILTER_VALIDATE_INT, ['options' => ['min_range' => 1]]);
    if ($iterations === false || $length === false || !in_array($masked, ['0', '1'], true)) {
        throw new InvalidArgumentException('Expected positive iterations, positive payload bytes, mask 0 or 1');
    }
    $pattern = implode('', array_map(chr(...), range(0, 255)));
    $payload = substr(str_repeat($pattern, intdiv($length + 255, 256)), 0, $length);
    $kind = $argv[4] ?? 'binary';
    $check = ($argv[5] ?? '') === 'check';
    if (!in_array($kind, ['binary', 'object', 'close', 'compressed'], true) || ($kind === 'close' && $length > 125)) {
        throw new InvalidArgumentException('Expected binary/object/close/compressed; close reasons <=125 bytes');
    }
    $flags = SWOOLE_WEBSOCKET_FLAG_FIN | ($masked === '1' ? SWOOLE_WEBSOCKET_FLAG_MASK : 0);
    $input = $payload;
    if ($kind === 'compressed') $flags |= SWOOLE_WEBSOCKET_FLAG_COMPRESS;
    if ($kind === 'object' || $kind === 'close') {
        $input = $kind === 'close' ? new Swoole\WebSocket\CloseFrame() : new Frame();
        $input->opcode = $kind === 'close' ? WEBSOCKET_OPCODE_CLOSE : WEBSOCKET_OPCODE_BINARY;
        $input->flags = $flags | ($kind === 'close' ? SWOOLE_WEBSOCKET_FLAG_COMPRESS : 0);
        $input->finish = false;
        if ($kind === 'close') { $input->code = 1000; $input->reason = $payload; }
        else $input->data = $payload;
    }
    $roundtrip = static function () use ($payload, $flags, $input, $kind): void {
        $packed = Frame::pack($input, WEBSOCKET_OPCODE_BINARY, $flags);
        if (!is_string($packed)) {
            throw new RuntimeException('Frame::pack did not return a string');
        }
        $frame = Frame::unpack($packed);
        $opcode = $kind === 'close' ? WEBSOCKET_OPCODE_CLOSE : WEBSOCKET_OPCODE_BINARY;
        $data = $kind === 'close' ? ($frame->reason ?? null) : ($frame->data ?? null);
        if (!$frame instanceof Frame || $data !== $payload || $frame->opcode !== $opcode ||
            $frame->finish !== ($kind !== 'object') ||
            (bool) ($frame->flags & SWOOLE_WEBSOCKET_FLAG_RSV1) !== ($kind === 'compressed') ||
            ($kind === 'close' && $frame->code !== 1000)) {
            throw new RuntimeException('Frame roundtrip returned incorrect payload, opcode, flags or close code');
        }
    };
    if ($check) {
        for ($i = 0; $i < $iterations; ++$i) $roundtrip();
        echo json_encode(['iterations' => $iterations, 'payload_bytes' => $length,
            'masked' => $masked === '1', 'kind' => $kind, 'verified' => $iterations, 'errors' => 0], JSON_THROW_ON_ERROR), "\n";
        exit(0);
    }
    $warmup = 200;
    for ($i = 0; $i < $warmup; ++$i) {
        $roundtrip();
    }
    $start = hrtime(true);
    for ($i = 0; $i < $iterations; ++$i) {
        $roundtrip();
    }
    $elapsed = hrtime(true) - $start;
    if ($elapsed <= 0) {
        throw new RuntimeException('Nonpositive measured duration');
    }
    echo json_encode([
        'kind' => $kind,
        'iterations' => $iterations, 'payload_bytes' => $length, 'masked' => $masked === '1',
        'warmup_roundtrips' => $warmup, 'elapsed_ns' => $elapsed, 'errors' => 0,
        'roundtrips_per_second' => $iterations * 1e9 / $elapsed,
        'ns_per_roundtrip' => $elapsed / $iterations,
    ], JSON_THROW_ON_ERROR), "\n";
} catch (Throwable $error) {
    fwrite(STDERR, $error::class . ': ' . $error->getMessage() . "\n");
    exit(1);
}
