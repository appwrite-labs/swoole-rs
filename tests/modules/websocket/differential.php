<?php
declare(strict_types=1);
use Swoole\WebSocket\Frame;

// Rejected control frames log a Swoole warning to stdout, which carries the JSON result.
swoole_async_set(['log_level' => SWOOLE_LOG_ERROR]);
set_error_handler(static function (int $severity, string $message): never {
    throw new ErrorException($message, 0, $severity);
});
$cases = [];
$errors = [];
foreach ([0, 1, 125, 126, 127, 65535, 65536] as $length) {
    $payload = substr(str_repeat(implode('', array_map(chr(...), range(0, 255))), intdiv($length + 255, 256)), 0, $length);
    foreach ([0, 1, 2, 8, 9, 10] as $opcode) {
        // Control frames have no extended payload; close includes a two-byte code.
        if (($opcode === 8 && $length > 123) || ($opcode >= 9 && $length > 125)) continue;
        // RSV1 is covered at the native seam; this PHP gate excludes compression.
        foreach ([1, 33] as $flags) {
            $id = "$length/$opcode/$flags";
            try {
                $input = $payload;
                $packed = Frame::pack($input, $opcode, $flags);
                $decoded = Frame::unpack($packed);
                if (!$decoded instanceof Frame || $decoded->opcode !== $opcode || !$decoded->finish) {
                    throw new RuntimeException('Incorrect decoded frame metadata');
                }
                if ($opcode !== 8 && $decoded->data !== $payload) {
                    throw new RuntimeException('Incorrect decoded payload');
                }
                if ($opcode === 8 && $decoded->reason !== $payload) {
                    throw new RuntimeException('Incorrect close reason');
                }
                $cases[$id] = [
                    'packed_sha256' => hash('sha256', $packed), 'packed_length' => strlen($packed),
                    'header_hex' => bin2hex(substr($packed, 0, min(strlen($packed), 14))),
                    'decoded_data_sha256' => hash('sha256', $decoded->data ?? ''),
                    'decoded_data_length' => strlen($decoded->data ?? ''),
                    'decoded_opcode' => $decoded->opcode, 'decoded_finish' => $decoded->finish,
                    'decoded_flags' => $decoded->flags,
                    'decoded_close_code' => $decoded->code ?? null,
                    'decoded_close_reason_sha256' => isset($decoded->reason) ? hash('sha256', $decoded->reason) : null,
                    'decoded_close_reason_length' => isset($decoded->reason) ? strlen($decoded->reason) : null,
                ];
            } catch (Throwable $error) {
                $errors[$id] = $error::class . ': ' . $error->getMessage();
            }
        }
    }
}
// Exercise actual zlib and Zend frame-property/lifetime callers. Explicit
// finish properties override flags before control normalization. The original
// 58 byte cases above remain intact.
foreach ([0, 1, 123] as $length) {
    $payload = substr(str_repeat("\x00\xffPHP/Rust ", 20), 0, $length);
    foreach ([0, 1, 2, 8, 9, 10] as $opcode) {
        foreach ([0, 1, 2, 3, 32, 33, 34, 35] as $flags) {
            foreach (['string', 'object-null', 'object-false', 'object-true'] as $kind) {
                $id = "policy/$kind/$length/$opcode/$flags";
                try {
                    $input = $payload;
                    $effective = $flags;
                    if ($kind !== 'string') {
                        $input = $opcode === 8 ? new Swoole\WebSocket\CloseFrame() : new Frame();
                        $input->opcode = $opcode;
                        $input->flags = $flags;
                        $input->finish = match ($kind) { 'object-false' => false, 'object-true' => true, default => null };
                        if ($opcode === 8) $input->reason = $payload;
                        else $input->data = $payload;
                        if ($input->finish !== null) $effective = ($flags & ~1) | (int) $input->finish;
                    }
                    // Contradictory arguments establish that object properties
                    // take precedence over the scalar arguments.
                    $packed = $kind === 'string' ? Frame::pack($input, $opcode, $flags) : Frame::pack($input, 15, 0);
                    $wireHash = hash('sha256', $packed);
                    $compressed = (bool) (ord($packed[0]) & 64);
                    $decoded = Frame::unpack($packed);
                    $inputAfterHash = hash('sha256', $packed);
                    if ($kind !== 'string' && $input->flags !== $flags) throw new RuntimeException('Pack mutated frame flags');
                    unset($input, $packed);
                    gc_collect_cycles();
                    $finish = $opcode >= 8 || (bool) ($effective & 1);
                    $needCompress = $length > 0 && ($opcode === 1 || $opcode === 2) && ($effective & 3) === 3;
                    if (!$decoded instanceof Frame || $decoded->opcode !== $opcode || $decoded->finish !== $finish ||
                        ($opcode === 8 ? $decoded->reason : $decoded->data) !== $payload || $compressed !== $needCompress) {
                        throw new RuntimeException('Incorrect PHP frame policy or payload lifetime');
                    }
                    $copy = clone $decoded;
                    if ($opcode === 8) $copy->reason .= 'changed';
                    else $copy->data .= 'changed';
                    if (($opcode === 8 ? $decoded->reason : $decoded->data) !== $payload) throw new RuntimeException('Clone aliased payload');
                    $cases[$id] = ['packed_sha256' => $wireHash, 'input_after_unpack_sha256' => $inputAfterHash,
                        'decoded_flags' => $decoded->flags, 'decoded_finish' => $decoded->finish, 'compressed' => $compressed];
                } catch (Throwable $error) {
                    $errors[$id] = $error::class . ': ' . $error->getMessage();
                }
            }
        }
    }
}
// The pinned sw_unset_bit macro clears COMPRESS but preserves a supplied
// RSV1 on controls. These deliberately malformed compression advertisements
// are pack-only: asserting a decompressed round trip would change input policy.
foreach ([1, 123, 125] as $length) {
    $payload = str_repeat('x', $length);
    foreach ([8, 9, 10] as $opcode) {
        foreach ([4, 5, 6, 7, 36, 37, 38, 39] as $flags) {
            foreach (['string', 'object'] as $kind) {
                $id = "control-rsv1/$kind/$length/$opcode/$flags";
                try {
                    $input = $payload;
                    if ($kind === 'object') {
                        $input = $opcode === 8 ? new Swoole\WebSocket\CloseFrame() : new Frame();
                        $input->opcode = $opcode;
                        $input->flags = $flags;
                        $input->finish = false;
                        if ($opcode === 8) $input->reason = $payload;
                        else $input->data = $payload;
                    }
                    $packed = Frame::pack($input, $opcode, $flags);
                    // Since v6.2.4 (c3e746b09) the encoder rejects control payloads over
                    // 125 bytes; a close frame adds a 2-byte code to its reason.
                    if ($opcode === 8 && $length > 123) {
                        if ($packed !== '') throw new RuntimeException('Oversized close frame was packed');
                        $cases[$id] = ['rejected' => true];
                        continue;
                    }
                    if (ord($packed[0]) !== (0xc0 | $opcode) ||
                        (bool) (ord($packed[1]) & 128) !== (bool) ($flags & 32)) {
                        throw new RuntimeException('Pinned control RSV1/mask/FIN policy changed');
                    }
                    $cases[$id] = ['packed_sha256' => hash('sha256', $packed), 'packed_length' => strlen($packed)];
                } catch (Throwable $error) {
                    $errors[$id] = $error::class . ': ' . $error->getMessage();
                }
            }
        }
    }
}
echo json_encode(['cases' => $cases, 'errors' => $errors], JSON_THROW_ON_ERROR), "\n";
exit($errors ? 1 : 0);
