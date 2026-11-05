<?php
$result = cpp_chain(function () {
    cpp_native_catch();
    cpp_publish();
});
$expected = [3, 0, 2, 8192, 8192 * 42, 1, true];
if ($result !== $expected) {
    fwrite(STDERR, json_encode(['expected' => $expected, 'observed' => $result]) . "\n");
    exit(1);
}
echo "cpp-chain: ok\n";
