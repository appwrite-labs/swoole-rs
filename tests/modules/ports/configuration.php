<?php
// Independent PHP/Zend boundary checks; original PHPTs remain unchanged.
function port_require($condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
function port_observe(array $value): void {
    echo 'OBS ', json_encode($value, JSON_THROW_ON_ERROR), "\n";
}
$case = $argv[1];
$server = new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE);
$port = $server->ports[0];
$initial = static function () {};
$replacement = static function () {};
$port->on('receive', $initial);
$port->set(['backlog'=>17]);

if (str_starts_with($case, 'fatal_')) {
    register_shutdown_function(static function () use ($port, $initial, $case) {
        $error = error_get_last();
        port_observe(['case'=>$case, 'shutdown'=>true, 'error_type'=>$error['type'] ?? null,
                      'raw_backlog'=>$port->setting['backlog'],
                      'callback_retained'=>$port->getCallback('receive') === $initial]);
    });
    $options = match ($case) {
        'fatal_empty_eof' => ['package_eof'=>''],
        'fatal_long_eof' => ['package_eof'=>'123456789'], // Bounded rejection, never an overflowing input.
        'fatal_length_type' => ['package_length_type'=>'?'],
        'fatal_length_offset' => ['package_length_offset'=>65535],
        'fatal_body_offset' => ['package_body_offset'=>65535],
        'fatal_body_alias' => ['package_body_start'=>65535],
    };
    $port->set(['backlog'=>19, ...$options]);
    throw new RuntimeException('expected configuration rejection did not occur');
}

if ($case === 'convert_eof' || $case === 'convert_length_type') {
    $value = new class($port, $replacement, $case) {
        public int $calls = 0;
        public function __construct(private $port, private $callback, private string $case) {}
        public function __toString(): string {
            $this->calls++;
            $this->port->on('receive', $this->callback);
            return $this->case === 'convert_eof' ? "\r\n" : 'N';
        }
    };
    $key = $case === 'convert_eof' ? 'package_eof' : 'package_length_type';
    $port->set([$key=>$value]);
    port_require($value->calls === 1, 'one string conversion');
    port_require($port->getCallback('receive') === $replacement, 'conversion reentry retained');
    port_require($port->setting[$key] === $value, 'original object published in settings');
    port_observe(['case'=>$case, 'conversions'=>$value->calls, 'reentry'=>true, 'raw_object_published'=>true]);
} elseif ($case === 'settings_release_reentry') {
    $released = 0;
    $value = new class($port, $replacement, $released) {
        private $released;
        public function __construct(private $port, private $callback, &$released) { $this->released =& $released; }
        public function __toString(): string { return 'X'; }
        public function __destruct() { $this->released++; $this->port->on('receive', $this->callback); }
    };
    $weak = WeakReference::create($value);
    $port->set(['package_eof'=>$value]);
    unset($value);
    port_require($released === 0 && $weak->get() !== null, 'setting retains native object');
    $port->set(['package_eof'=>"\r\n"]);
    port_require($released === 1 && $weak->get() === null, 'overwritten setting released once');
    port_require($port->getCallback('receive') === $replacement, 'setting destructor reentry retained');
    port_observe(['case'=>$case, 'released'=>$released, 'reentry'=>true]);
} elseif ($case === 'callback_release_reentry') {
    $released = 0;
    $value = new class($port, $replacement, $released) {
        private $released;
        public function __construct(private $port, private $callback, &$released) { $this->released =& $released; }
        public function __destruct() { $this->released++; $this->port->on('receive', $this->callback); }
    };
    $weak = WeakReference::create($value);
    $callback = static function () use ($value) {};
    $port->on('packet', $callback);
    unset($callback, $value);
    port_require($released === 0 && $weak->get() !== null, 'callback keeps captured object');
    $port->on('packet', static function () {});
    port_require($released === 1 && $weak->get() === null, 'callback capture released once');
    port_require($port->getCallback('receive') === $replacement, 'callback destructor reentry retained');
    port_observe(['case'=>$case, 'released'=>$released, 'reentry'=>true]);
} elseif ($case === 'ignored_ssl') {
    $value = new class {
        public int $calls = 0;
        public function __toString(): string { $this->calls++; throw new RuntimeException('ignored SSL conversion executed'); }
    };
    $port->set(['ssl_ciphers'=>$value, 'ssl_client_cert_file'=>$value]);
    port_require($value->calls === 0, 'non-SSL port does not convert SSL options');
    port_require($port->setting['ssl_ciphers'] === $value, 'ignored setting still published');
    port_observe(['case'=>$case, 'conversions'=>0, 'raw_object_published'=>true]);
} elseif ($case === 'conversion_throw') {
    $value = new class($port, $replacement) {
        public int $calls = 0;
        public function __construct(private $port, private $callback) {}
        public function __toString(): string {
            $this->calls++;
            $this->port->on('receive', $this->callback);
            throw new RuntimeException('port conversion sentinel');
        }
    };
    // Upstream reports the conversion's exception as uncaught, then rejects the
    // failed (empty) length type as fatal; observe the state at shutdown (#317).
    register_shutdown_function(static function () use ($port, $replacement, $value, $case) {
        $error = error_get_last();
        port_observe(['case'=>$case, 'shutdown'=>true, 'error_type'=>$error['type'] ?? null,
                      'conversions'=>$value->calls, 'reentry'=>$port->getCallback('receive') === $replacement,
                      'setting_merged'=>array_key_exists('package_length_type', $port->setting)]);
    });
    $port->set(['package_length_type'=>$value]);
    throw new RuntimeException('expected configuration rejection did not occur');
} else {
    throw new RuntimeException('unknown case');
}
