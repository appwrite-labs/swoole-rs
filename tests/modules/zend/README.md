# Zend tests

`cargo test -p swoole-rs-zend --test allocator` exercises the production `Emalloc`
implementation with a checked allocation shim: zero-sized and unsupported-alignment
requests, one boxed drop/raw handoff, and Vec growth/shrink with preserved contents
and no outstanding allocations. It compiles the allocator source without linking the Zend crate, so PHP-enabled
workspace builds cannot pull in the real allocator alongside the shim. It does not
run Zend itself.

`run.py` compares real PHP warning-handler bailouts and memory-limit behavior
against pinned upstream. Timer/coroutine/stdlib original PHPTs exercise the request
allocator's callable arguments, mock fiber contexts and stdext call records.
