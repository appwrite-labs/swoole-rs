<?php
// Read-only token/reflection census. Never evaluate stub source or invoke API methods.
// A default that names a constant is recorded by name; evaluating one can need an absent extension.
function swr_inventory_default(ReflectionParameter $p): array {
    if (!$p->isDefaultValueAvailable()) { return ['available' => false]; }
    try { return ['available' => true, 'value' => $p->getDefaultValue()]; }
    catch (Error $error) { return ['available' => true, 'unevaluated' => get_class($error)]; }
}
function swr_inventory_class_constants(ReflectionClass $class): array {
    $out = [];
    foreach ($class->getReflectionConstants() as $constant) {
        $out[$constant->getName()] = ['declared_by' => $constant->getDeclaringClass()->getName(),
            'visibility' => $constant->isPublic() ? 'public' : ($constant->isProtected() ? 'protected' : 'private'),
            'final' => $constant->isFinal(),
            'type' => method_exists($constant, 'getType') ? (string) $constant->getType() : null];
    }
    ksort($out);
    return $out;
}
if (($argv[1] ?? '') === '--tokens') {
    $out = [];
    foreach (json_decode(stream_get_contents(STDIN), true, 512, JSON_THROW_ON_ERROR) as $path => $source) {
        $out[$path] = array_map(static fn($t) => is_array($t) ? [token_name($t[0]), $t[1], $t[2]] : ['char', $t, 0], token_get_all($source));
    }
} else {
    if (!extension_loaded('swoole')) { throw new RuntimeException('swoole not loaded'); }
    $describe = static function (ReflectionFunctionAbstract $f) {
        return ['name' => $f->getName(), 'return_type' => (string) $f->getReturnType(), 'returns_reference' => $f->returnsReference(),
            'parameters' => array_map(static fn($p) => ['name' => $p->getName(), 'type' => (string) $p->getType(),
                'optional' => $p->isOptional(),
                'default_constant' => $p->isDefaultValueAvailable() && $p->isDefaultValueConstant() ? $p->getDefaultValueConstantName() : null,
                'default' => swr_inventory_default($p), 'variadic' => $p->isVariadic(), 'by_reference' => $p->isPassedByReference()], $f->getParameters())];
    };
    $classes = [];
    foreach (array_merge(get_declared_classes(), get_declared_interfaces(), get_declared_traits()) as $name) {
        $c = new ReflectionClass($name);
        if ($c->getExtensionName() !== 'swoole' && stripos($name, 'Swoole\\') !== 0) { continue; }
        $methods = [];
        foreach ($c->getMethods() as $m) { $methods[$m->getName()] = $describe($m) + ['declared_by' => $m->getDeclaringClass()->getName(), 'static' => $m->isStatic(), 'visibility' => $m->isPublic() ? 'public' : ($m->isProtected() ? 'protected' : 'private'), 'abstract' => $m->isAbstract(), 'final' => $m->isFinal()]; }
        ksort($methods);
        $properties = [];
        foreach ($c->getProperties() as $p) {
            $properties[$p->getName()] = ['declared_by' => $p->getDeclaringClass()->getName(),
                'type' => (string) $p->getType(), 'static' => $p->isStatic(), 'readonly' => $p->isReadOnly(),
                'visibility' => $p->isPublic() ? 'public' : ($p->isProtected() ? 'protected' : 'private')];
        }
        ksort($properties);
        $classes[$name] = ['canonical_name' => $c->getName(), 'provider' => $c->getExtensionName() ?: 'embedded_or_userland',
            'source_file' => $c->getFileName() ?: null, 'parent' => ($c->getParentClass() ?: null)?->getName(),
            'interfaces' => $c->getInterfaceNames(), 'methods' => $methods, 'properties' => $properties,
            'abstract' => $c->isAbstract(), 'final' => $c->isFinal(), 'modifiers' => $c->getModifiers(),
            'default_properties' => $c->getDefaultProperties(), 'constants' => $c->getConstants(), 'constant_metadata' => swr_inventory_class_constants($c)];
    }
    ksort($classes);
    $functions = [];
    foreach (array_merge(...array_values(get_defined_functions())) as $name) {
        $f = new ReflectionFunction($name);
        if ($f->getExtensionName() === 'swoole' || stripos($name, 'Swoole\\') === 0) { $functions[$name] = $describe($f) + ['provider' => $f->getExtensionName() ?: 'embedded_or_userland']; }
    }
    ksort($functions);
    $constants = get_defined_constants(true)['swoole'] ?? []; ksort($constants);
    $out = ['php' => PHP_VERSION, 'zts' => PHP_ZTS, 'os' => PHP_OS, 'integer_bytes' => PHP_INT_SIZE,
        'classes' => $classes, 'functions' => $functions, 'constants' => $constants,
        'ini' => ini_get_all('swoole', false), 'loaded_extensions' => get_loaded_extensions()];
}
echo json_encode($out, JSON_THROW_ON_ERROR | JSON_INVALID_UTF8_SUBSTITUTE | JSON_PRETTY_PRINT), "\n";
