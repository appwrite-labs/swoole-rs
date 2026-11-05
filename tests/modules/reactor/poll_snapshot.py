#!/usr/bin/env python3
"""Positive poll snapshot storage ABI, original translation and target libc layout gate.

No callback, lifetime/fd-reuse, zero-codec, fault injection or network probes.
This does not execute full original reactor/core suites or establish parity.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from ordered_gate import ROOT, PIN, block, hybrid


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--archive', type=Path, required=True)
    p.add_argument('--report', type=Path, required=True)
    p.add_argument('--sanitize', action='store_true')
    a = p.parse_args()
    original = {name: subprocess.check_output(['git', '-C', str(a.source), 'show', PIN+':'+name], text=True)
                for name in ['src/reactor/poll.cc', 'include/swoole_reactor.h']}
    selected_tree, abi = hybrid(a.archive)
    a.report.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='swr-poll-snapshot-', dir=a.report.parent) as tmp:
        tree = Path(tmp)
        selected = {name: (selected_tree/name).read_text() for name in original}
        fragments = []
        for label, sources in [('Original', original), ('Selected', selected)]:
            source = sources['src/reactor/poll.cc']
            name = 'Snapshot' + label
            fields = ('swr_v1_reactor_poll_snapshot *snapshot_;\n' if label == 'Selected' else '') + 'pollfd *events_; Socket **event_sockets_; int max_events_;'
            methods = []
            if label == 'Selected':
                if 'swr_v1_reactor_state_open' not in source or 'swr_v1_reactor_state_wait' not in source:
                    raise ValueError('Production poll must use the Rust backend owner and wait')
                methods = [
                    name+'::'+name+'(Reactor *reactor,int count):ReactorImpl(reactor) { if(count<0) throw std::bad_array_new_length(); snapshot_=swr_v1_reactor_poll_new(count); if(!snapshot_) throw std::bad_alloc(); events_=swr_v1_reactor_poll_data(snapshot_); event_sockets_=reinterpret_cast<Socket **>(swr_v1_reactor_poll_sockets(snapshot_)); max_events_=count; reactor_->max_event_num=count; }',
                    name+'::~'+name+'(){ swr_v1_reactor_poll_free(snapshot_); }',
                    'int '+name+'::set_events() const { return swr_v1_reactor_poll_fill(snapshot_,reactor_->registry); }',
                ]
            else:
                for signature in ['ReactorPoll::ReactorPoll(', 'ReactorPoll::~ReactorPoll(', 'int ReactorPoll::set_events() const']:
                    method, _ = block(source, source.index(signature))
                    method = method.replace('ReactorPoll', name).replace('translate_events_to_poll(', 'translate_events_to_poll_'+label.lower()+'(')
                    if label == 'Selected':
                        method = method.replace('reactor_->get_sockets()', 'reactor_->registry')
                    methods.append(method)
            fragments.append('struct '+name+' : ReactorImpl { '+fields+'\n'+name+'(Reactor*, int); ~'+name+'(); int set_events() const; };\n'+'\n'.join(methods))
            # Upstream defines the translations in poll.cc; the hybrid header inlines its Rust calls.
            for signature in ['int16_t translate_events_to_poll(', 'int translate_events_from_poll(']:
                text = next(text for text in sources.values() if signature in text)
                method, _ = block(text, text.index(signature))
                method = method.replace('translate_events_to_poll(', 'translate_events_to_poll_'+label.lower()+'(').replace('translate_events_from_poll(', 'translate_events_from_poll_'+label.lower()+'(')
                fragments.append(method)
        (tree/'poll-snapshot.inc').write_text('\n'.join(fragments))
        executable = tree/'snapshot'
        cmd = ['c++', '-std=c++14', '-O1', '-g', '-I'+str(tree), abi, str(ROOT/'tests/modules/reactor/poll_snapshot.cc'), str(a.archive.resolve()), '-lpthread', '-o', str(executable)]
        if a.sanitize:
            cmd += ['-fsanitize=address,undefined', '-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'):
            cmd += ['-ldl', '-lm']
        result = subprocess.run(cmd, capture_output=True, text=True)
        report = dict(pin=PIN, hybrid_tree=str(selected_tree), compiler=subprocess.check_output(['c++', '--version'], text=True).splitlines()[0], command=cmd, compile_returncode=result.returncode, compile_stdout=result.stdout, compile_stderr=result.stderr, scope='original constructor/destructor/set_events/translation bodies and selected storage API used by the Rust backend owner; original over a std map, selected over the Rust registry; actual libc pipe/poll; not full reactor or callback qualification')
        if result.returncode == 0:
            result = subprocess.run([str(executable)], capture_output=True, text=True)
            report.update(returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)
            if result.returncode == 0:
                report['observations'] = json.loads(result.stdout)
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(json.dumps(report, indent=2)+'\n')
        print(json.dumps({k: report[k] for k in ['compile_returncode', 'observations'] if k in report}))
        return result.returncode


if __name__ == '__main__':
    raise SystemExit(main())
