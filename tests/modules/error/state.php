<?php
// Actual PHP binding, fork and shutdown checks; no network services or timing.
if (!function_exists('pcntl_fork')) {
    fwrite(STDERR, "UNSUPPORTED: pcntl_fork is required\n");
    exit(2);
}
$log = $argv[1];
swoole_async_set(['log_file' => $log, 'log_level' => SWOOLE_LOG_NOTICE]);
function emitError(int $code, string $token): void {
    swoole_error_log_ex(SWOOLE_LOG_NOTICE, $code, '#M11:' . $token . '#');
}
function joinChild(int $pid): void {
    if ($pid < 0 || pcntl_waitpid($pid, $status) !== $pid ||
        !pcntl_wifexited($status) || pcntl_wexitstatus($status) !== 0) {
        throw new RuntimeException('Child failed');
    }
}
// A first insertion in a child must not publish a handle in its empty parent.
$pid = pcntl_fork();
if ($pid === 0) {
    swoole_ignore_error(34567);
    emitError(34567, 'empty-child-hidden');
    exit(0);
}
joinChild($pid);
emitError(34567, 'empty-parent-visible');
$codes = array_merge(range(1, 4096), [-2147483648, -1, 0, 2147483647]);
foreach ($codes as $code) {
    swoole_ignore_error($code);
    swoole_ignore_error($code);
    emitError($code, 'ignored-' . $code);
}
// Zend integers narrow to the native int32 domain at the unchanged binding.
emitError(4294967295, 'negative-alias-hidden');
emitError(4294967296, 'zero-alias-hidden');
$role = 'parent';
register_shutdown_function(function () use (&$role): void {
    emitError(0, $role . '-shutdown-hidden');
    emitError(123456789, $role . '-shutdown-visible');
});
$pid = pcntl_fork();
if ($pid === 0) {
    $role = 'child';
    swoole_ignore_error(76543);
    foreach ($codes as $code) {
        emitError($code, 'inherited-' . $code);
    }
    emitError(76543, 'child-add-hidden');
    exit(0);
}
joinChild($pid);
emitError(76543, 'parent-unmodified-visible');
emitError(-2147483647, 'signed-miss-visible');
// Parent shutdown runs after output; the outer driver checks the final log.
echo json_encode(['insertions' => count($codes) * 2, 'forks' => 2]);
