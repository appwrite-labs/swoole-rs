#!/usr/bin/env python3
"""Compile the complete Scheduler TU against a verified completed PHP-helper SDK capture."""
import argparse
import json
import shutil
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import require_build_space
from tools.harness import run_isolated, write_report
from gate import sha, stage
from overlay import body, header_selected, HEADER_DEST, HEADER_SHA

SCHEDULER_EXPORTS = {'swr_v1_phpcoroutines_scheduler_' + suffix for suffix in ('prepare_slot', 'try_push_slot', 'next_slot', 'drop_slot', 'phase')} | {'swr_v1_phpcoroutines_options', 'swr_v1_phpcoroutines_configure_owned', 'swr_v1_phpcoroutines_scheduler_control', 'swr_v1_phpcoroutines_repeat'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--sdk-report', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    work = output.parent / ('phpcoroutines-sdk-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, work=str(work), commands=[], scope=__doc__,
                  runtime_executed=False, measurement_performed=False,
                  limits=['Compilation and import/layout observations only; no PHP/callback/original execution.',
                          'Captured native (pinned) and hybrid-build headers; phpcoroutines-only is the pinned profile with this module\'s overlay.'])
    try:
        header_source = (args.source / HEADER_DEST).read_text()
        selected_header = header_selected(header_source)
        layout = 'namespace swoole {\nstruct OriginalPHPContextLayout {' + body(header_source, 'struct PHPContext {') + '};\n'
        layout += 'static_assert(sizeof(PHPContext) == sizeof(OriginalPHPContextLayout), "context size");\n'
        layout += 'static_assert(alignof(PHPContext) == alignof(OriginalPHPContextLayout), "context alignment");\n'
        fields = 'bailout vm_stack_top vm_stack_end vm_stack vm_stack_page_size execute_data jit_trace_num error_handling exception_class exception output_ptr serialize_lock serialize unserialize in_silence enable_scheduler ori_error_reporting tmp_error_reporting co fci fci_cache return_value fiber_context fiber_init_notified defer_tasks on_yield on_resume on_close pcid context last_msec switch_count'.split()
        for field in fields:
            layout += f'static_assert(offsetof(PHPContext, {field}) == offsetof(OriginalPHPContextLayout, {field}), "context {field}");\n'
        layout += '#ifdef ZEND_CHECK_STACK_LIMIT\n'
        for field in ('stack_base', 'stack_limit'):
            layout += f'static_assert(offsetof(PHPContext, {field}) == offsetof(OriginalPHPContextLayout, {field}), "context {field}");\n'
        layout += '#endif\n}\n'
        report['context_layout_members'] = fields + ['stack_base (conditional)', 'stack_limit (conditional)']
        sdk = json.loads(args.sdk_report.read_text())
        if not sdk['passed'] or not sdk['layout_matched']:
            raise ValueError('SDK prerequisite did not pass')
        rows = [row for row in sdk['commands'] if row['unit'] == 'ext-src/php_swoole_cxx.cc'
                and not row['stdext_enabled'] and row['engine'] in ('native', 'hybrid')]
        if {(row['engine'], row['cpp_standard']) for row in rows} != {
                (engine, std) for engine in ('native', 'hybrid') for std in ('c++14', 'c++17')}:
            raise ValueError('Incomplete captured SDK profiles')
        dependencies = {name: digest for row in rows for name, digest in row['dependencies_sha256'].items()}
        for name, digest in dependencies.items():
            if sha(name) != digest:
                raise ValueError('Changed captured SDK dependency: ' + name)
        for destination in ('ext-src/swoole_coroutine_scheduler.cc', 'ext-src/swoole_coroutine.cc', 'ext-src/swoole_coroutine_system.cc'):
            original, selected = stage(args.source, work / ('overlay-' + Path(destination).stem), destination)
            exports = (SCHEDULER_EXPORTS if destination.endswith('_scheduler.cc') else {
                'swr_v1_phpcoroutines_yield_registry_' + name for name in ('try_put', 'take', 'erase', 'drop')} | {'swr_v1_phpcoroutines_create', 'swr_v1_phpcoroutines_stats', 'swr_v1_phpcoroutines_activation'} | {'swr_v1_phpcoroutines_defer_' + name for name in ('new', 'push', 'pop', 'drop')} | {'swr_v1_phpcoroutines_autoload_' + suffix for suffix in ('owner_try_new', 'is_owner', 'try_push', 'pop', 'drop', 'policy')})
            if destination.endswith('_system.cc'):
                exports = {'swr_v1_phpcoroutines_system_' + name for name in ('file', 'sleep', 'address', 'complete')}
            exports |= {'swr_v1_phpcoroutines_bootstrap', 'swr_v1_phpcoroutines_method'}
            if not destination.endswith('_system.cc'): exports |= {'swr_v1_phpcoroutines_config'}
            if destination.endswith('_scheduler.cc'):
                exports |= {'swr_v1_phpcoroutines_callback_control'}
            if destination.endswith('swoole_coroutine.cc'):
                exports |= {'swr_v1_phpcoroutines_context_route', 'swr_v1_phpcoroutines_context_query', 'swr_v1_phpcoroutines_trace', 'swr_v1_phpcoroutines_interrupt', 'swr_v1_phpcoroutines_stack_step', 'swr_v1_phpcoroutines_fiber', 'swr_v1_phpcoroutines_scheduler_phase', 'swr_v1_phpcoroutines_exit', 'swr_v1_phpcoroutines_metadata', 'swr_v1_phpcoroutines_repeat'} | {'swr_v1_phpcoroutines_join_' + suffix for suffix in ('try_new', 'clone', 'drop', 'try_insert', 'erase_empty', 'empty', 'next', 'release_membership', 'cancel', 'can_resume')}
            for feature in ('native', 'phpcoroutines-only', 'hybrid'):
                profile = work / Path(destination).stem / feature
                profile.mkdir(parents=True)
                source = profile / Path(destination).name
                # Pointer type changes preserve every SchedulerObject field position.
                text = original if feature == 'native' else selected if feature == 'phpcoroutines-only' else (Path(sdk['hybrid_source']) / destination).read_text()
                source.write_text(text + ('''
    struct OriginalSchedulerObjectLayout { void *list; bool started; zend_object std; };
    static_assert(sizeof(SchedulerObject) == sizeof(OriginalSchedulerObjectLayout), "SchedulerObject size");
    static_assert(alignof(SchedulerObject) == alignof(OriginalSchedulerObjectLayout), "SchedulerObject alignment");
    static_assert(offsetof(SchedulerObject, started) == offsetof(OriginalSchedulerObjectLayout, started), "started offset");
    static_assert(offsetof(SchedulerObject, std) == offsetof(OriginalSchedulerObjectLayout, std), "Zend offset");
    #ifdef SWOOLE_RS_PHPCOROUTINES
    static_assert(sizeof(swr_phpcoroutines_scheduler_phase) == sizeof(bool), "started size");
    static_assert(alignof(swr_phpcoroutines_scheduler_phase) == alignof(bool), "started alignment");
    static_assert(sizeof(swr_phpcoroutines_scheduler_callback_owner) == sizeof(void *), "callback identity cell size");
    static_assert(alignof(swr_phpcoroutines_scheduler_callback_owner) == alignof(void *), "callback identity cell alignment");
    constexpr swr_phpcoroutines_scheduler_callback_owner constant_callback_seed{0};
    static_assert(constant_callback_seed.identity == 0, "callback constant zero seed");
    #endif
    ''' if destination.endswith('_scheduler.cc') else ''))
                if destination == 'ext-src/swoole_coroutine.cc':
                    source.write_text(source.read_text() + layout + """
struct OriginalPublicConfigLayout {
    uint64_t max_num;
    uint32_t hook_flags;
    bool enable_preemptive_scheduler;
    bool enable_deadlock_check;
};
static_assert(sizeof(PHPCoroutine::Config) == sizeof(OriginalPublicConfigLayout), "public Config size");
static_assert(alignof(PHPCoroutine::Config) == alignof(OriginalPublicConfigLayout), "public Config alignment");
static_assert(offsetof(PHPCoroutine::Config, max_num) == offsetof(OriginalPublicConfigLayout, max_num), "public maximum offset");
static_assert(offsetof(PHPCoroutine::Config, hook_flags) == offsetof(OriginalPublicConfigLayout, hook_flags), "public hooks offset");
static_assert(offsetof(PHPCoroutine::Config, enable_preemptive_scheduler) == offsetof(OriginalPublicConfigLayout, enable_preemptive_scheduler), "public preemption offset");
static_assert(offsetof(PHPCoroutine::Config, enable_deadlock_check) == offsetof(OriginalPublicConfigLayout, enable_deadlock_check), "public deadlock offset");
#ifdef SWOOLE_RS_PHPCOROUTINES
static_assert(sizeof(swr_phpcoroutines_activation_state) == sizeof(bool), "activated size");
static_assert(alignof(swr_phpcoroutines_activation_state) == alignof(bool), "activated alignment");
static_assert(sizeof(swr_phpcoroutines_interrupt_state) == sizeof(bool), "interrupt size");
static_assert(alignof(swr_phpcoroutines_interrupt_state) == alignof(bool), "interrupt alignment");
constexpr swr_phpcoroutines_interrupt_state constant_interrupt_seed{0};
static_assert(constant_interrupt_seed.running == 0, "constant interrupt seed");
constexpr swr_phpcoroutines_activation_state constant_activated_seed{0};
static_assert(constant_activated_seed.started == 0, "constant false seed");
static_assert(sizeof(swr_phpcoroutines_config_state) == sizeof(OriginalPublicConfigLayout), "selected config ABI slot size");
static_assert(alignof(swr_phpcoroutines_config_state) == alignof(OriginalPublicConfigLayout), "selected config ABI alignment");
#endif
#ifdef SWOOLE_RS_PHPCOROUTINES
// The private Native owner record is gone; only mandatory Native RAII handle
// remains. This source-local representation is not part of public PHPContext ABI.
static_assert(sizeof(AutoloadContextQueue) == sizeof(void *), "selected opaque autoload handle size");
static_assert(alignof(AutoloadContextQueue) == alignof(void *), "selected opaque autoload handle alignment");
#else
struct OriginalAutoloadQueueLayout { Coroutine *coroutine; void *queue; };
static_assert(sizeof(AutoloadQueue) == sizeof(OriginalAutoloadQueueLayout), "autoload owner size");
static_assert(alignof(AutoloadQueue) == alignof(OriginalAutoloadQueueLayout), "autoload owner alignment");
static_assert(offsetof(AutoloadQueue, queue) == offsetof(OriginalAutoloadQueueLayout, queue), "autoload queue offset");
#endif
""")
                for standard in ('c++14', 'c++17'):
                    row = next(row for row in rows if row['engine'] == ('hybrid' if feature == 'hybrid' else 'native')
                               and row['cpp_standard'] == standard)
                    captured_cxx = [Path(name) for name in row['dependencies_sha256'] if name.endswith('/ext-src/php_swoole_cxx.h')]
                    if len(captured_cxx) != 1:
                        raise ValueError('Ambiguous captured header root')
                    captured_headers = captured_cxx[0].parent.parent
                    # The hybrid mirror already holds the hybrid build's headers.
                    staged_headers = captured_headers if feature == 'hybrid' else work / ('headers-' + feature + '-' + standard)
                    if not staged_headers.exists():
                        require_build_space(ROOT)
                        shutil.copytree(captured_headers, staged_headers)
                        if sha(staged_headers / HEADER_DEST) != HEADER_SHA:
                            raise ValueError('Unexpected captured coroutine header identity')
                        if feature != 'native':
                            (staged_headers / HEADER_DEST).write_text(selected_header)
                    configs = [Path(name) for name, digest in row['dependencies_sha256'].items()
                               if Path(name).name == 'config.h' and digest == row['config_sha256']]
                    if len(configs) != 1:
                        raise ValueError('Ambiguous captured configuration')
                    config = configs[0]
                    (profile / 'config.h').write_text(config.read_text() + '\n#undef SWOOLE_RS_PHPCOROUTINES\n' +
                        ('#define SWOOLE_RS_PHPCOROUTINES 1\n' if feature != 'native' else ''))
                    shutil.copyfile(config.parent / 'php_swoole.h', profile / 'php_swoole.h')
                    dep, obj = profile / (standard + '.d'), profile / (standard + '.o')
                    command = [part.replace(str(captured_headers), str(staged_headers)) for part in row['command']]
                    command[command.index('-fsyntax-only')] = '-c'
                    command[command.index('-MF') + 1] = str(dep)
                    command[-1] = str(source)
                    command[command.index('-I' + str(config.parent))] = '-I' + str(profile)
                    command += ['-O2', '-Wall', '-Werror', *([] if feature == 'hybrid' else ['-I' + str(ROOT / 'native/phpcoroutines')]), '-o', str(obj)]
                    unit_exports = set(exports)
                    capabilities = None
                    if destination == 'ext-src/swoole_coroutine.cc':
                        # Observe the ACTUAL preprocessing profile. PHP 8.2 can
                        # lack this Native caller; never force/assume capability.
                        probe = []; skip = False
                        for part in command:
                            if skip: skip = False; continue
                            if part in ('-MF', '-o'): skip = True; continue
                            if part in ('-MD', '-MMD'): continue
                            probe.append('-E' if part == '-c' else part)
                        probe.append('-dM')
                        require_build_space(ROOT)
                        observed = run_isolated(probe, timeout=120)
                        report.setdefault('capability_commands', []).append(dict(
                            unit=destination, feature=feature, standard=standard,
                            command=probe, exit=observed.returncode, stdout=observed.stdout, stderr=observed.stderr))
                        write_report(output, report)
                        if observed.returncode: raise ValueError('Native capability preprocessing failed')
                        macros = {line.split()[1] for line in observed.stdout.splitlines() if line.startswith('#define ')}
                        capabilities = dict(native_thread_context='SW_USE_THREAD_CONTEXT' in macros,
                                            coroutine_time='SW_CORO_TIME' in macros,
                                            apple='__APPLE__' in macros)
                        report.setdefault('conditional_method_registration', []).append(dict(
                            feature=feature, standard=standard, method='getExecuteTime',
                            outcome='COMPILED' if capabilities['coroutine_time'] else 'UNSUPPORTED',
                            reason='Actual Native SW_CORO_TIME macro presence; never forced for SDK qualification'))
                    require_build_space(ROOT)
                    result = run_isolated(command, timeout=120)
                    entry = dict(unit=destination, feature=feature, standard=standard, command=command, exit=result.returncode,
                                 stdout=result.stdout, stderr=result.stderr, capabilities=capabilities)
                    report['commands'].append(entry)
                    write_report(output, report)
                    if result.returncode:
                        raise ValueError('Complete Scheduler TU failed compilation')
                    symbols = run_isolated(['nm', '-u', str(obj)], timeout=30)
                    entry['undefined_symbols'] = dict(exit=symbols.returncode, stdout=symbols.stdout, stderr=symbols.stderr)
                    if symbols.returncode:
                        raise ValueError('Object import observation failed')
                    imports = {line.split()[-1].lstrip('_') for line in symbols.stdout.splitlines() if line.split()}
                    actual = {name for name in imports if name.startswith('swr_v1_phpcoroutines_')}
                    if actual != (set() if feature == 'native' else unit_exports):
                        raise ValueError('Unexpected/missing Scheduler Rust imports: ' + repr(actual))
                    entry['phpcoroutines_imports'] = sorted(actual)
        if any(sha(name) != digest for name, digest in dependencies.items()):
            raise ValueError('Captured SDK dependencies changed during compilation')
        report.update(passed=True, object_layout_assertions_passed=True)
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ('passed', 'error', 'work')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
