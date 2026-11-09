<?php
foreach (['', 'a', 'swoole', str_repeat('x', 100), "a\0b"] as $data) {
    s('hashcode', fn() => swoole_hashcode($data));
    s('hashcode.one_at_a_time', fn() => swoole_hashcode($data, 1));
    s('hashcode.unknown', fn() => swoole_hashcode($data, 2));
}
s('mime.count', fn() => count(swoole_mime_type_list()));
s('mime.sorted', fn() => mimes());
s('mime.exists', fn() => swoole_mime_type_exists('html'));
s('mime.exists.missing', fn() => swoole_mime_type_exists('swrdiff'));
s('mime.get', fn() => swoole_mime_type_get('index.html'));
s('mime.get.path', fn() => swoole_mime_type_get('/a/b.c/d.JSON'));
s('mime.get.none', fn() => swoole_mime_type_get('noext'));
s('mime.get.dot', fn() => swoole_mime_type_get('trailing.'));
s('mime.get.missing', fn() => swoole_mime_type_get('x.swrdiff'));
s('mime.add', fn() => swoole_mime_type_add('swrdiff', 'application/x-swrdiff'));
s('mime.add.again', fn() => swoole_mime_type_add('swrdiff', 'text/plain'));
s('mime.get.added', fn() => swoole_mime_type_get('x.swrdiff'));
s('mime.set', fn() => swoole_mime_type_set('swrdiff', 'text/x-swrdiff'));
s('mime.set.new', fn() => swoole_mime_type_set('swrnew', 'text/x-swrnew'));
s('mime.get.set', fn() => swoole_mime_type_get('x.swrdiff'));
// The list's order is the hash map's; its contents are the contract.
function mimes(): string { $list = swoole_mime_type_list(); sort($list); return md5(implode("\n", $list)); }
s('mime.list', fn() => [in_array('text/x-swrdiff', swoole_mime_type_list(), true), in_array('text/x-swrnew', swoole_mime_type_list(), true)]);
s('mime.count.added', fn() => count(swoole_mime_type_list()));
s('mime.delete', fn() => swoole_mime_type_delete('swrdiff'));
s('mime.delete.again', fn() => swoole_mime_type_delete('swrdiff'));
s('mime.exists.deleted', fn() => swoole_mime_type_exists('swrdiff'));
s('mime.add.empty', fn() => swoole_mime_type_add('', 'text/plain'));
s('mime.exists.empty', fn() => swoole_mime_type_exists(''));
s('implicit.refcount', fn() => swoole_implicit_fn('refcount', new stdClass));
s('implicit.func_handler.missing', fn() => swoole_implicit_fn('func_handler', 'swr_no_such_function'));
s('implicit.unknown', fn() => swoole_implicit_fn('swr_unknown'));
$o = new stdClass;
s('object_by_handle', fn() => swoole_get_object_by_handle(spl_object_id($o)) === $o);
foreach ([0, -1, PHP_INT_MAX, 1 << 20] as $handle) {
    s('object_by_handle.invalid', fn() => swoole_get_object_by_handle($handle));
}
s('objects', fn() => in_array($o, swoole_get_objects(), true));
s('vm_status', fn() => array_map('gettype', swoole_get_vm_status()));
s('name_resolver.add.other', fn() => swoole_name_resolver_add(new stdClass));
s('name_resolver.remove.other', fn() => swoole_name_resolver_remove(new stdClass));
$resolver = new class('http://127.0.0.1:1') extends Swoole\NameResolver {
    public function join(string $name, string $ip, int $port, array $options = []): bool { return false; }
    public function leave(string $name, string $ip, int $port): bool { return false; }
    public function getCluster(string $name): ?Swoole\NameResolver\Cluster { return null; }
    public function lookup(string $name) { return $name === 'swr.cluster' ? null : "10.0.0.1"; }
};
s('name_resolver.remove.unadded', fn() => swoole_name_resolver_remove($resolver));
s('name_resolver.add', fn() => swoole_name_resolver_add($resolver));
s('name_resolver.lookup', fn() => swoole_name_resolver_lookup('swr.example', new Swoole\NameResolver\Context()));
s('name_resolver.lookup.null', fn() => swoole_name_resolver_lookup('swr.cluster', new Swoole\NameResolver\Context(AF_INET, true)));
s('name_resolver.remove', fn() => swoole_name_resolver_remove($resolver));
s('name_resolver.remove.again', fn() => swoole_name_resolver_remove($resolver));
s('name_resolver.lookup.removed', fn() => swoole_name_resolver_lookup('localhost', new Swoole\NameResolver\Context()));
// Without swoole.profile and swoole.leak_detection the tracer refuses.
s('tracer.prof_end.unstarted', fn() => swoole_tracer_prof_end('/swr/never-written.json'));
s('tracer.prof_begin', fn() => swoole_tracer_prof_begin());
s('tracer.prof_begin.options', fn() => swoole_tracer_prof_begin(['x' => 1]));
s('tracer.prof_end', fn() => swoole_tracer_prof_end('/swr/never-written.json'));
s('tracer.leak_detect', fn() => swoole_tracer_leak_detect());
s('tracer.leak_detect.threshold', fn() => swoole_tracer_leak_detect(1));
