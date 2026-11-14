<?php
// Real Zend conversions and recursive reply formatting; no substitute encoder.
use Swoole\Redis\Server;

$events = [];
set_error_handler(function ($severity, $message) use (&$events) {
    $events[] = ['warning', $severity, $message];
    return true;
});
class ReplyText {
    public function __construct(private string $text, private bool $fail = false) {}
    public function __toString(): string {
        global $events;
        $events[] = ['convert', $this->text];
        if ($this->fail) {
            throw new RuntimeException('conversion:'.$this->text);
        }
        return $this->text;
    }
}
$cases = [];
foreach ([Server::NIL, Server::ERROR, Server::STATUS, Server::INT, Server::STRING, Server::SET, Server::MAP, -17] as $type) {
    $cases[] = ['missing-'.$type, [$type]];
}
foreach ([null, false, true, 0, -1, PHP_INT_MIN, PHP_INT_MAX, 1.5, '', 'hello', "a\0b", "a\r\nb", new ReplyText('object'), NAN, INF, -INF, 1e100, [], ['array'], new stdClass()] as $i => $value) {
    foreach ([Server::NIL, Server::ERROR, Server::STATUS, Server::INT, Server::STRING] as $type) {
        $cases[] = ['scalar-'.$i.'-'.$type, [$type, $value]];
    }
}
foreach ([[], [null, false, true, -2, ''], ['binary'=>"a\0b", "k\0tail"=>'value', -1=>'minus'],
          [[1, 2], ['key'=>[null, 'nested']]], [new ReplyText('first'), new ReplyText('second')],
          [new ReplyText('before'), new ReplyText('fail', true), new ReplyText('after')]] as $i => $value) {
    foreach ([Server::SET, Server::MAP] as $type) {
        $cases[] = ['array-'.$i.'-'.$type, [$type, $value]];
    }
}
$cases[] = ['bulk-2MiB', [Server::STRING, str_repeat("a\0b\r\n", 419430)]];
$results = [];
foreach ($cases as [$name, $args]) {
    $events = [];
    try {
        $value = Server::format(...$args);
        $result = ['name'=>$name, 'length'=>strlen($value), 'sha256'=>hash('sha256', $value)];
        if (strlen($value) < 256) {
            $result['base64'] = base64_encode($value);
        }
    } catch (Throwable $error) {
        $result = ['name'=>$name, 'exception'=>get_class($error), 'message'=>$error->getMessage()];
    }
    $result['events'] = $events;
    $results[] = $result;
}
echo json_encode($results, JSON_THROW_ON_ERROR|JSON_INVALID_UTF8_SUBSTITUTE), "\n";
