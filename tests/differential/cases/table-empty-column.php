<?php
$t = s('table.new', fn() => new Swoole\Table(8));
s('table.column', fn() => $t->column('', Swoole\Table::TYPE_INT));
s('table.column', fn() => $t->column('s', Swoole\Table::TYPE_STRING, 8));
s('table.create', fn() => $t->create());
s('table.set', fn() => $t->set('k', ['' => 5, 's' => 'v']));
s('table.incr', fn() => $t->incr('k2', 'missing', 1));
s('table.get', fn() => $t->get('k'));
s('table.get', fn() => $t->get('k', ''));
s('table.iterate', fn() => iterator_to_array($t));
