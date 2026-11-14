<?php
// Independent valid-input public Port API witness. Original PHPTs are separate.
function ensure_port($value, string $message): void {
    if (!$value) { throw new RuntimeException($message); }
}
$server = new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE);
$port = $server->ports[0];
$events = ['connect','receive','close','packet','request','handshake','beforehandshakeresponse','open','message','disconnect','bufferfull','bufferempty'];
$callbacks = [];
foreach ($events as $event) {
    $callback = static function (...$args) {};
    ensure_port($port->on(strtoupper($event), $callback) === true, 'register '.$event);
    ensure_port($port->getCallback($event) === $callback, 'lookup '.$event);
    ensure_port($port->getCallback(strtoupper($event)) === $callback, 'folded lookup '.$event);
    $callbacks[$event] = $callback;
}
$warnings = [];
set_error_handler(static function ($level, $message) use (&$warnings) { $warnings[] = $message; return true; });
foreach (['', 'unknown', "connect\0x"] as $unknown) {
    ensure_port($port->on($unknown, static function () {}) === false, 'unknown event rejected');
    ensure_port($port->getCallback($unknown) === null, 'unknown callback absent');
}
restore_error_handler();
ensure_port(count($warnings) === 3, 'three unknown event warnings');
ensure_port($port->getCallback('receive') === $callbacks['receive'], 'unknown did not replace callback');
$replacement = static function () {};
$name = new class($port, $replacement) {
    public int $conversions = 0;
    public function __construct(private $port, private $callback) {}
    public function __toString(): string {
        $this->conversions++;
        $this->port->on('receive', $this->callback);
        return 'ReCeIvE';
    }
};
ensure_port($port->getCallback($name) === $replacement, 'observe callback after native conversion reentry');
ensure_port($name->conversions === 1, 'one native name conversion');
unset($name);
class PortOwnedToken {
    public static int $destroyed = 0;
    public function __destruct() { self::$destroyed++; }
}
$token = new PortOwnedToken;
$owned = static function () use ($token) {};
$port->on('packet', $owned);
unset($token, $owned);
ensure_port(PortOwnedToken::$destroyed === 0, 'native property/callback keep token alive');
$port->on('packet', static function () {});
ensure_port(PortOwnedToken::$destroyed === 1, 'replacement releases prior native callable/property');
$settings = ['backlog'=>32, 'socket_buffer_size'=>1048576, 'heartbeat_idle_time'=>5,
             'buffer_high_watermark'=>65536, 'buffer_low_watermark'=>16384,
             'package_length_type'=>'N', 'package_length_offset'=>0,
             'package_body_offset'=>4, 'package_max_length'=>1048576];
$port->set($settings);
foreach ($settings as $key=>$value) { ensure_port($port->setting[$key] === $value, 'setting publication '.$key); }
echo json_encode(['registered_events'=>count($events), 'unknown_warnings'=>count($warnings),
                  'conversion_reentry'=>true, 'replaced_token_destroyed'=>PortOwnedToken::$destroyed,
                  'settings_published'=>count($settings), 'socket_optional'=>['available'=>method_exists($port,'getSocket'),'status'=>'unmeasured'],
                  'server_started'=>false], JSON_THROW_ON_ERROR), "\n";
