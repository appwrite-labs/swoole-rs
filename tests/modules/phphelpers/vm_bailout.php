<?php
if (!function_exists('swoole_typed_array')) { fwrite(STDERR,"UNSUPPORTED: stdext required\n");exit(2); }
$mode=$argv[1];
if ($mode==='pcre-exit' || $mode==='json-exit') {
    set_error_handler(function () use ($mode): void { echo "exit\n";exit($mode==='pcre-exit' ? 37 : 39); });
    echo "before\n";
    if ($mode==='pcre-exit') swoole_str_match('subject','/');
    else swoole_str_json_decode('{}',0);
} elseif ($mode==='vm-exit') {
    class VmExitPayload {
        public static bool $first=true;
        public function __destruct() { if(self::$first) { self::$first=false;echo "exit\n";exit(38); } }
    }
    $array=swoole_typed_array('<VmExitPayload>',[new VmExitPayload()]);
    echo "before\n";
    $array[0]=new VmExitPayload();
}
echo "UNREACHABLE\n";
exit(1);
