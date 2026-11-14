<?php
$table = new Swoole\Table(64);
$table->column('count', Swoole\Table::TYPE_INT);
$table->column('label', Swoole\Table::TYPE_STRING, 32);
$table->create();
$table->set('job', ['count' => 1, 'label' => 'queued']);
$child = new Swoole\Process(static function () use ($table): void {
    $table->incr('job', 'count', 4);
    $table->set('result', ['count' => 42, 'label' => 'complete']);
    exit(0);
}, false, 0);
$child->start();
$wait = Swoole\Process::wait(true);
if ($wait === false || $wait['code'] !== 0 || $wait['signal'] !== 0) {
    throw new RuntimeException('Child process failed');
}
$job = $table->get('job');
$result = $table->get('result');
$beforeDelete = count($table);
$deleted = $table->del('job');
$missing = $table->exists('job');
$afterDelete = count($table);
$table->destroy();
echo json_encode(compact('job', 'result', 'beforeDelete', 'deleted', 'missing', 'afterDelete')), "\n";
