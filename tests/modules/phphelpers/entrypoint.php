<?php
// Supplemental real Zend named-call entry; no host files, ports or network.
function check(bool $ok, string $message): void { if (!$ok) throw new RuntimeException($message); }
check(function_exists('m26_named_call'), 'test-only named helper loaded');
check(m26_named_call('strlen', "bin\0ary") === 7, 'builtin binary argument');
$shared = 4;
function m26_named_reference(array $box): array { $box[0] += 3; return $box; }
$result = m26_named_call('m26_named_reference', [&$shared]);
check($shared === 7 && $result === [7], 'returned array preserves nested reference');
unset($result);
$events = [];
class M26NamedLifetime {
    public function __construct(public int $identity) {}
    public function __destruct() {
        global $events;
        $events[] = ['destroy', $this->identity, m26_named_call('strlen', 'reentry')];
    }
}
function m26_named_object(): object { return new M26NamedLifetime(19); }
$object = m26_named_call('m26_named_object');
check($object instanceof M26NamedLifetime && $object->identity === 19, 'returned object custody');
unset($object);
check($events === [['destroy', 19, 7]], 'one native object destruction with named reentry');
$autoload = [];
spl_autoload_register(function (string $class) use (&$autoload): void {
    if ($class === 'M26NamedAutoload') {
        $autoload[] = m26_named_call('strtoupper', 'autoload');
        class M26NamedAutoload { public static function value(int $value): int { return $value + 5; } }
    }
});
check(m26_named_call('M26NamedAutoload::value', 3) === 8, 'native callable class autoload');
check($autoload === ['AUTOLOAD'], 'autoload reenters named entry');
function m26_named_yield(): string {
    Swoole\Coroutine::sleep(0.001);
    return m26_named_call('strtoupper', 'coroutine');
}
$yielded = null;
Swoole\Coroutine\run(function () use (&$yielded): void { $yielded = m26_named_call('m26_named_yield'); });
check($yielded === 'COROUTINE', 'native callback suspension and nested named return');
$known = m26_known_cache();
check($known === ['count' => 6, 'matches' => 6, 'interned' => 6], 'the known strings C++ reads match its header and are interned');
$ownedEvents = [];
$ownedIdentity = 0;
class M26OwnedCallable {
    public function __construct(public int $identity) {}
    public function __invoke(): void {}
    public function __destruct() {
        global $ownedEvents;
        $ownedEvents[] = [$this->identity, m26_named_call('strtoupper', 'release')];
    }
}
function m26_owned_factory(): object {
    global $ownedIdentity;
    return new M26OwnedCallable(++$ownedIdentity);
}
check(m26_owned_callable('m26_owned_factory'), 'owned callable direct release');
check($ownedEvents === [[1, 'RELEASE']], 'direct sole-reference Callable destructor reentry');
Swoole\Coroutine\run(function (): void { check(m26_owned_callable('m26_owned_factory'), 'owned callable coroutine release'); });
check($ownedEvents === [[1, 'RELEASE'], [2, 'RELEASE']], 'coroutine sole-reference Callable destructor reentry');
$regexSubject = 'A1 B22 C333';
$regexPattern = '/([A-Z])(\d+)/';
preg_match($regexPattern, $regexSubject, $expectedMatches);
check(swoole_str_match($regexSubject, $regexPattern) === $expectedMatches, 'PCRE single default options');
preg_match($regexPattern, $regexSubject, $expectedMatches, PREG_OFFSET_CAPTURE, 3);
check(swoole_str_match($regexSubject, $regexPattern, PREG_OFFSET_CAPTURE, 3) === $expectedMatches, 'PCRE optional flags and offset');
preg_match_all($regexPattern, $regexSubject, $expectedMatches, PREG_SET_ORDER, 3);
check(swoole_str_match_all($regexSubject, $regexPattern, PREG_SET_ORDER, 3) === $expectedMatches, 'PCRE multiple matches and offset');
$regexWarnings = [];
set_error_handler(function (int $severity, string $message) use (&$regexWarnings): bool {
    $regexWarnings[] = $severity;
    return true;
});
try { check(swoole_str_match('abc', '/[/') === false, 'native failed regex compilation returns false'); }
finally { restore_error_handler(); }
check($regexWarnings === [E_WARNING], 'one native regex compilation warning');
set_error_handler(function (): never { throw new RuntimeException('M26 regex warning reentry'); });
$regexException = null;
try { swoole_str_match('abc', '/[/'); }
catch (RuntimeException $error) { $regexException = $error->getMessage(); }
finally { restore_error_handler(); }
check($regexException === 'M26 regex warning reentry', 'native throwing warning handler');
check(swoole_array_search([10, 20], 20) === array_search(20, [10, 20]), 'array search binding');
check(swoole_array_contains([10, 20], 20) === in_array(20, [10, 20]), 'array contains binding');
check(swoole_array_join(['a', 'b'], '-') === implode('-', ['a', 'b']), 'array join binding');
check(swoole_array_key_exists(['k' => 1], 'k') === array_key_exists('k', ['k' => 1]), 'array key binding');
$increment = fn(int $value): int => $value + 1;
check(swoole_array_map([1, 2], $increment) === array_map($increment, [1, 2]), 'array map binding');
check(swoole_array_replace_str(['ab', 'ca'], 'a', 'x') === str_replace('a', 'x', ['ab', 'ca']), 'array replace binding');
check(swoole_array_ireplace_str(['Ab', 'ca'], 'a', 'x') === str_ireplace('a', 'x', ['Ab', 'ca']), 'array insensitive replace binding');
check(swoole_str_split('a,b', ',') === explode(',', 'a,b'), 'string split binding');
check(swoole_hash('abc', 'sha256') === hash('sha256', 'abc'), 'hash binding');
check(swoole_str_replace('abc', 'a', 'x') === str_replace('a', 'x', 'abc'), 'string replace binding');
check(swoole_str_ireplace('Abc', 'a', 'x') === str_ireplace('a', 'x', 'Abc'), 'string insensitive replace binding');
$jsonObject = '{"a":[1,2],"b":{"ok":true}}';
check(swoole_str_json_decode($jsonObject) === json_decode($jsonObject, true), 'JSON associative policy');
$decodedObject = swoole_str_json_decode_to_object($jsonObject, 512, JSON_OBJECT_AS_ARRAY);
check($decodedObject instanceof stdClass && $decodedObject == json_decode($jsonObject, false), 'JSON object mode overrides associative option');
$largeJson = '{"large":123456789012345678901234567890}';
$jsonFlags = JSON_BIGINT_AS_STRING | JSON_THROW_ON_ERROR;
check(swoole_str_json_decode($largeJson, 512, $jsonFlags) === json_decode($largeJson, true, 512, $jsonFlags), 'JSON unrelated options preserved');
check(swoole_str_json_decode('{') === null && json_last_error() === JSON_ERROR_SYNTAX, 'native JSON syntax error and global status');
$jsonException = null;
try { swoole_str_json_decode_to_object('{', 512, JSON_THROW_ON_ERROR); }
catch (JsonException $error) { $jsonException = $error->getCode(); }
check($jsonException === JSON_ERROR_SYNTAX && json_last_error() === JSON_ERROR_SYNTAX, 'native throwing JSON path retains prior error status');
$jsonWarnings = [];
set_error_handler(function (int $severity, string $message) use (&$jsonWarnings): bool {
    $jsonWarnings[] = [$severity, str_contains($message, 'Depth must be greater than zero')];
    return true;
});
try { check(swoole_str_json_decode($jsonObject, 0) === null, 'native JSON zero-depth return'); }
finally { restore_error_handler(); }
check($jsonWarnings === [[E_WARNING, true]], 'native JSON depth warning');
$merged = m26_merge_strings([
    ['color', 'red'], ['color', 'green'], ['color', 'blue'],
    ['7', 'seven'], ['07', 'leading'], ['7', 'again'],
    ["bin\0key", "value\0one"], ["bin\0key", 'two'],
]);
check($merged === [
    'color' => ['red', 'green', 'blue'],
    7 => ['seven', 'again'], '07' => 'leading',
    "bin\0key" => ["value\0one", 'two'],
], 'actual native merge string promotion, append and explicit key lengths');
check(m26_merge_strings([]) === [], 'empty native merge sequence');
$boolEvents = [];
foreach ([false, true] as $capture) {
    $called = m26_bool_call(function () use (&$boolEvents): array {
        $boolEvents[] = 'direct';
        return ['returned' => 9];
    }, false, $capture);
    $expectedCall = ['success' => true, 'capture' => $capture, 'value_defined' => $capture];
    if ($capture) $expectedCall['value'] = ['returned' => 9];
    check($called === $expectedCall, 'actual bool entry direct capture mode');
}
Swoole\Coroutine\run(function () use (&$boolEvents): void {
    foreach ([false, true] as $capture) {
        $before = count($boolEvents);
        $called = m26_bool_call(function () use (&$boolEvents): int {
            $boolEvents[] = 'coroutine';
            return 11;
        }, true, $capture);
        $expectedCall = ['success' => true, 'capture' => $capture, 'value_defined' => $capture];
        if ($capture) $expectedCall['value'] = null;
        check($called === $expectedCall, 'actual bool entry coroutine ignores callback result');
        check(count($boolEvents) === $before + 1, 'non-suspending child completes before native Callable scope ends');
    }
});
check($boolEvents === ['direct', 'direct', 'coroutine', 'coroutine'], 'one callback per bool entry mode');
echo json_encode(['matches' => 36, 'bool_call_modes' => $boolEvents, 'merge_string_sequences' => 2, 'json_exception_code' => $jsonException, 'json_warnings' => $jsonWarnings, 'public_wrapper_bindings' => 11, 'regex_warnings' => $regexWarnings, 'regex_exception' => $regexException, 'known_cache' => $known, 'owned_callable_releases' => $ownedEvents, 'destructors' => $events, 'autoload' => $autoload, 'yielded' => $yielded], JSON_THROW_ON_ERROR), "\n";
