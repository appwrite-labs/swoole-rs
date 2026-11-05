<?php
use Swoole\Thread;
class ServerExitDestructor {
    public function __destruct() { swoole_implicit_fn('bailout', 235); }
}
if (Thread::getInfo()['is_main_thread']) {
    $mode = $argv[1];
    $after = new Thread\Atomic(0);
    $thread = new Thread(__FILE__, $mode, $after);
    if (!$thread->join()) throw new RuntimeException('join failed');
    $expected = $mode === 'fatal' ? 255 : (in_array($mode, ['custom', 'destructor'], true) ? 235 : 0);
    if ($thread->getExitStatus() !== $expected || $after->get() !== ($mode === 'normal' ? 1 : 0)) {
        throw new RuntimeException('Server callback exit/post-effect behavior changed');
    }
    echo json_encode([$mode, $thread->getExitStatus(), $after->get()]), "\n";
    exit;
}
[$mode, $after] = Thread::getArguments();
error_reporting(0);
swr_actual_server_guard(function () use ($mode) {
    if ($mode === 'fatal') trigger_error('Server guard probe fatal', E_USER_ERROR);
    if ($mode === 'custom') swoole_implicit_fn('bailout', 235);
    if ($mode === 'destructor') return new ServerExitDestructor();
});
$after->add();
