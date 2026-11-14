#!/usr/bin/env python3
"""Pinned declaration/build census and optional actual configured PHP reflection."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT, parse_engines, write_report
from tools.build_upstream import COMMIT


def declarations(tokens):
    rows = []
    namespace, owner, pending = '', None, None
    stack = []
    significant = [(kind, text, line) for kind, text, line in tokens if kind not in {'T_WHITESPACE', 'T_COMMENT', 'T_DOC_COMMENT'}]
    for i, (kind, text, line) in enumerate(significant):
        if kind == 'T_NAMESPACE':
            j = i + 1
            parts = []
            while significant[j][1] not in {'{', ';'}:
                parts.append(significant[j][1]); j += 1
            name = ''.join(parts).strip('\\')
            if significant[j][1] == ';':
                namespace = name
            else:
                pending = ('namespace', name)
        elif kind in {'T_CLASS', 'T_INTERFACE', 'T_TRAIT', 'T_ENUM'} and (i == 0 or significant[i - 1][0] != 'T_DOUBLE_COLON'):
            if significant[i + 1][0] != 'T_STRING':
                raise ValueError('Anonymous class in declaration inventory requires explicit handling')
            name = '\\'.join(x for x in [namespace, significant[i + 1][1]] if x)
            rows.append({'kind': kind[2:].lower(), 'name': name, 'line': line})
            pending = ('class', name)
        elif kind == 'T_FUNCTION':
            j = i + 1
            while significant[j][1] == '&':
                j += 1
            if significant[j][0] == 'T_STRING':
                name = significant[j][1]
                rows.append({'kind': 'method' if owner else 'function',
                             'name': owner + '::' + name if owner else '\\'.join(x for x in [namespace, name] if x), 'line': line})
        elif kind == 'T_CONST':
            # Names are immediately before each top-level '='; nested commas
            # and '=' inside literal text cannot create declarations. Imports
            # ('use const ...') have no initializer and contribute no rows.
            depth = 0
            for j in range(i + 1, len(significant)):
                _, value, _ = significant[j]
                if value in {';', '}'} and depth == 0:
                    break
                if value == '=' and depth == 0:
                    previous_kind, name, name_line = significant[j - 1]
                    if previous_kind != 'T_STRING':
                        raise ValueError('Unparsed stub constant declaration')
                    rows.append({'kind': 'class_constant' if owner else 'constant',
                                 'name': owner + '::' + name if owner else '\\'.join(x for x in [namespace, name] if x),
                                 'line': name_line})
                if value in {'(', '[', '{'}: depth += 1
                elif value in {')', ']', '}'}: depth -= 1
        if text == '{':
            stack.append((namespace, owner))
            if pending:
                if pending[0] == 'namespace': namespace = pending[1]
                else: owner = pending[1]
                pending = None
        elif text == '}':
            if not stack: raise ValueError('Unbalanced stub scope')
            namespace, owner = stack.pop()
    if stack: raise ValueError('Unclosed stub scope')
    return rows


def blobs(source, pin, paths):
    data = subprocess.check_output(['git', '-C', str(source), 'cat-file', '--batch'],
        input=''.join(f'{pin}:{p}\n' for p in paths).encode())
    position, result = 0, {}
    for path in paths:
        end = data.index(b'\n', position)
        identity, kind, size = data[position:end].decode().split()
        if kind != 'blob': raise ValueError('Missing pinned blob: ' + path)
        size = int(size); value = data[end + 1:end + 1 + size]
        if hashlib.sha1(b'blob ' + str(size).encode() + b'\0' + value).hexdigest() != identity:
            raise ValueError('Pinned blob integrity failure: ' + path)
        result[path] = value
        position = end + 1 + size + 1
    return result


def census(source, php='php', root=ROOT, php_extensions=()):
    pin = COMMIT
    names = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', '--name-only', pin], text=True).splitlines()
    paths = sorted(p for p in names if p.endswith('.stub.php') or p == 'config.m4' or p == 'README.md'
                   or p.startswith('.github/workflows/') and p.endswith('.yml'))
    values = blobs(source, pin, paths)
    stubs = {p: b.decode() for p, b in values.items() if p.endswith('.stub.php')}
    extensions = [arg for name in php_extensions for arg in ['-d', f'extension={name}']]
    token_run = subprocess.run([php, '-n', *extensions, str(root/'tools/php_api_inventory.php'), '--tokens'],
                              input=json.dumps(stubs), capture_output=True, text=True, timeout=30)
    if token_run.returncode:
        raise ValueError('PHP tokenizer failed (load tokenizer via --php-extension if dynamic): ' + token_run.stdout + token_run.stderr)
    tokens = json.loads(token_run.stdout)
    modules = json.loads((root/'migration/modules.json').read_text())['modules']
    def owner(path):
        owners = [m['id'] for m in modules if any(path == t.rstrip('/') or path.startswith(t.rstrip('/') + '/') for t in m['upstream_patch_targets'])]
        return owners[0] if owners else None
    rows = []
    for path in sorted(stubs):
        native = 'ext-src/' + Path(path).name.replace('php_', '').replace('.stub.php', '.cc')
        for row in declarations(tokens[path]):
            rows.append({**row, 'source': path, 'candidate_native_source': native if native in names else None,
                         'module': owner(native), 'human_assignee': '@loks0n', 'behavior_status': 'unvalidated',
                         'evidence_target': 'original PHPT plus API/feature-specific actual configured checks; allocation pending where module is null'})
    config = values['config.m4'].decode()
    switches = [{'kind': k.lower(), 'name': name, 'line': config[:match.start()].count('\n') + 1,
                 'human_assignee': '@loks0n', 'owner': 'M00', 'status': 'unvalidated'}
                for match in re.finditer(r'PHP_ARG_(ENABLE|WITH)\(\[([^\]]+)\]', config)
                for k, name in [match.groups()]]
    if len(switches) != len(re.findall(r'(?m)^\s*PHP_ARG_(?:ENABLE|WITH)\(', config)):
        raise ValueError('Unparsed configure switch declaration')
    return {'schema_version': 1, 'upstream': {'commit': pin},
            'migration_manifest_sha256': hashlib.sha256((root/'migration/modules.json').read_bytes()).hexdigest(),
            'tool_sha256': {p: hashlib.sha256((root/p).read_bytes()).hexdigest() for p in ['tools/capability_inventory.py', 'tools/php_api_inventory.php']},
            'tokenizer_php': subprocess.check_output([php, '-n', '-r', 'echo PHP_VERSION;'], text=True),
            'units': {'stub_declarations': 'All pinned stub named class/interface/trait/enum/function/method/constant declarations, not a complete runtime API union',
                      'configure_switches': 'PHP_ARG_ENABLE/WITH source declarations, not enabled feature support'},
            'declarations': rows, 'configure_switches': switches,
            'source_sha256': {p: hashlib.sha256(b).hexdigest() for p, b in values.items()},
            'platform_and_version_source': [{'path': p, 'line': i + 1, 'text': line.strip()}
                for p, b in values.items() for i, line in enumerate(b.decode().splitlines())
                if p == 'README.md' and re.search(r'PHP 8\.|Linux,|WSL', line)
                or p == 'config.m4' and re.search(r'SW_OS=|SW_PHP_VERSION.*-(?:ge|lt)|PHP_ZTS|ZEND_THREAD_SAFE', line)],
            'limitations': ['Source census does not execute tests or prove Rust conversion/parity/performance.',
                'Stub declarations omit inherited APIs, aliases, native C/Zend constant registration, dynamic registration and embedded companion library; use configured reflection too.',
                'Candidate native filename is naming evidence only; null module means unallocated, not unsupported or excluded.',
                'Raw source conditions and CI hashes remain part of the denominator; no optional build, PHP version or OS is waived.',
                'External dependencies/minimal PHP ABI retention policy and unobserved configuration matrix remain open.']}



def compare_api_surfaces(observations):
    """Compare same-runtime loaded API facts; exactly two diagnostic constants differ."""
    if len(observations) != 2:
        raise ValueError('API comparison requires exactly two actual engines')
    native = [row for row in observations if row['engine']['rust_bridge_abi'] is None]
    selected = [row for row in observations if row['engine']['rust_bridge_abi'] is not None]
    if len(native) != 1 or len(selected) != 1:
        raise ValueError('API comparison requires native and bridge-selected engines')
    native, selected = native[0], selected[0]
    keys = ['php', 'zts', 'os', 'integer_bytes']
    if any(native['api'][key] != selected['api'][key] for key in keys):
        raise ValueError('API comparison requires the same PHP/platform/thread profile')
    marker_names = ['SWOOLE_RS_ABI_VERSION', 'SWOOLE_RS_FEATURE_BITS']
    markers = {key: selected['api']['constants'].get(key) for key in marker_names}
    if (any(key in native['api']['constants'] for key in marker_names)
            or markers['SWOOLE_RS_ABI_VERSION'] != selected['engine']['rust_bridge_abi']
            or markers['SWOOLE_RS_FEATURE_BITS'] != selected['engine']['rust_feature_bits']):
        raise ValueError('Bridge marker constants do not match actual engine metadata')
    left, right = native['api'], dict(selected['api'])
    right['constants'] = {key: value for key, value in right['constants'].items() if key not in marker_names}
    differences = []
    def walk(a, b, path):
        if isinstance(a, dict) and isinstance(b, dict):
            for key in sorted(a.keys() | b.keys()):
                child = path + [key]
                if key not in a or key not in b:
                    differences.append({'path': child, 'native_present': key in a, 'selected_present': key in b})
                else:
                    walk(a[key], b[key], child)
        elif a != b:
            differences.append({'path': path, 'native': a, 'selected': b})
    walk(left, right, [])
    return {'passed': not differences, 'differences': differences, 'diagnostic_constants': markers,
            'scope': 'Loaded reflection facts only; no method execution, attributes, complete configuration union or behavior/parity/performance proof.'}


def socket_availability(api):
    """PHP sockets and Swoole's optional client adapter are independent facts."""
    clients = [row for name, row in api['classes'].items()
               if row.get('canonical_name', name).casefold() == 'swoole\\client']
    return {
        'php_sockets_extension_loaded': 'sockets' in {name.casefold() for name in api['loaded_extensions']},
        'swoole_client_get_socket_method': any(
            name.casefold() == 'getsocket' and method['visibility'] == 'public'
            for client in clients for name, method in client['methods'].items()),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--engine', action='append')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--require-same-api', action='store_true', help='Compare two same-runtime native/selected loaded reflection surfaces')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = census(args.source, args.php, php_extensions=args.php_extension)
    report['configured_reflection'] = []
    for engine in parse_engines(args.engine, args.php, args.php_extension) if args.engine else []:
        command = engine.command(ROOT/'tools/php_api_inventory.php')
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        row = {'engine': engine.metadata(), 'command': command, 'exit': result.returncode,
               'stdout': result.stdout, 'stderr': result.stderr,
               'scope': 'Loaded API availability/signatures only; not a behavior test or union of all supported configurations'}
        report['configured_reflection'].append(row)
        if result.returncode or result.stderr:
            report['error'] = 'Reflection execution failed or warned; first output retained'
            write_report(args.output, report)
            return 1
        try:
            row['api'] = json.loads(result.stdout)
            row['socket_availability'] = socket_availability(row['api'])
        except ValueError as error:
            report['error'] = 'Reflection output is not JSON: ' + str(error)
            write_report(args.output, report)
            return 1
    if args.require_same_api:
        try:
            report['api_comparison'] = compare_api_surfaces(report['configured_reflection'])
        except ValueError as error:
            report['api_comparison'] = {'passed': False, 'error': str(error)}
    write_report(args.output, report)
    print(json.dumps({'stub_declarations': len(report['declarations']), 'configure_switches': len(report['configure_switches']),
                      'configured_engines': len(report['configured_reflection']), 'api_comparison_passed': report.get('api_comparison', {}).get('passed'), 'output': str(args.output)}))
    return int(args.require_same_api and not report['api_comparison']['passed'])


if __name__ == '__main__':
    sys.exit(main())
