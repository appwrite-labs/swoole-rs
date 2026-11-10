"""corpus/syscalls.py: strace lines become stable signatures, and only unexplained, repeatable differences fail."""
from collections import Counter
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

from corpus import syscalls
from corpus.syscalls import differences, explain, signature, signatures

# Lines as strace 6.8 (Ubuntu 24.04) writes them with -ff -s 0: one process per file, so no
# <... resumed> halves, only a call cut off by the process's death.
TRACE = '''\
socket(AF_INET, SOCK_STREAM|SOCK_CLOEXEC|SOCK_NONBLOCK, IPPROTO_IP) = 3
setsockopt(3, SOL_SOCKET, SO_REUSEADDR, [1], 4) = 0
bind(3, {sa_family=AF_INET, sin_port=htons(41234), sin_addr=inet_addr("127.0.0.1")}, 16) = 0
listen(3, 512)                          = 0
epoll_create1(EPOLL_CLOEXEC)            = 4
epoll_ctl(4, EPOLL_CTL_ADD, 3, {events=EPOLLIN, data={u32=3, u64=3}}) = 0
epoll_wait(4, [{events=EPOLLIN, data={u32=3, u64=3}}], 4096, -1) = 1
accept4(3, {sa_family=AF_INET, sin_port=htons(51022), sin_addr=inet_addr("127.0.0.1")}, [16], SOCK_CLOEXEC|SOCK_NONBLOCK) = 5
fcntl(5, F_GETFL)                       = 0x802 (flags O_RDWR|O_NONBLOCK)
setsockopt(5, SOL_TCP, TCP_NODELAY, [1], 4) = 0
recvfrom(5, ""..., 65536, 0, NULL, NULL) = 78
recvfrom(5, 0x7f3a1c000b70, 65536, 0, NULL, NULL) = -1 EAGAIN (Resource temporarily unavailable)
sendto(5, ""..., 132, MSG_NOSIGNAL, NULL, 0) = 132
sendmsg(5, {msg_name=NULL, msg_namelen=0, msg_iov=[{iov_base=""..., iov_len=12}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, MSG_NOSIGNAL) = 12
sendfile(5, 6, [0] => [1024], 1024)     = 1024
ioctl(5, FIONBIO, [1])                  = 0
ioctl(1, TCGETS, 0x7ffd5d1a3c40)        = -1 ENOTTY (Inappropriate ioctl for device)
getsockopt(5, SOL_SOCKET, SO_ERROR, [0], [4]) = 0
shutdown(5, SHUT_WR)                    = 0
close(5)                                = 0
rt_sigaction(SIGPIPE, {sa_handler=SIG_IGN, sa_mask=[], sa_flags=SA_RESTORER, sa_restorer=0x7f3a1d842520}, NULL, 8) = 0
clone(child_stack=NULL, flags=CLONE_CHILD_CLEARTID|CLONE_CHILD_SETTID|SIGCHLD, child_tidptr=0x7f3a1d9a5a10) = 4242
clone3({flags=CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|CLONE_SYSVSEM|CLONE_SETTLS|CLONE_PARENT_SETTID|CLONE_CHILD_CLEARTID, child_tid=0x7f3a1c9ff910, parent_tid=0x7f3a1c9ff910, exit_signal=0, stack=0x7f3a1c1ff000, stack_size=0x7ffe80, tls=0x7f3a1c9ff640} => {parent_tid=[4243]}, 88) = 4243
wait4(-1, [{WIFEXITED(s) && WEXITSTATUS(s) == 0}], WNOHANG, NULL) = 4242
--- SIGCHLD {si_signo=SIGCHLD, si_code=CLD_EXITED, si_pid=4242, si_uid=1001, si_status=0, si_utime=0, si_stime=0} ---
epoll_wait(4, 0x55d0c1a2b3c0, 4096, 500) = ? ERESTARTSYS (To be restarted if SA_RESTART is set)
epoll_wait(4,  <unfinished ...>)        = ?
+++ killed by SIGTERM +++
'''

EXPECTED = [
    'socket(AF_INET, SOCK_STREAM|SOCK_CLOEXEC|SOCK_NONBLOCK, IPPROTO_IP) = ok',
    'setsockopt(SOL_SOCKET, SO_REUSEADDR, [1]) = ok',
    'bind({sa_family=AF_INET, sin_port=htons(PORT), sin_addr=inet_addr("127.0.0.1")}) = ok',
    'listen(512) = ok',
    'epoll_create1(EPOLL_CLOEXEC) = ok',
    'epoll_ctl(EPOLL_CTL_ADD, {events=EPOLLIN}) = ok',
    'epoll_wait() = ok',
    'accept4(SOCK_CLOEXEC|SOCK_NONBLOCK) = ok',
    'fcntl(F_GETFL) = ok',
    'setsockopt(SOL_TCP, TCP_NODELAY, [1]) = ok',
    'recvfrom(0) = ok',
    'recvfrom(0) = EAGAIN',
    'sendto(MSG_NOSIGNAL) = ok',
    'sendmsg(MSG_NOSIGNAL) = ok',
    'sendfile() = ok',
    'ioctl(FIONBIO, [1]) = ok',
    'ioctl(TCGETS) = ENOTTY',
    'getsockopt(SOL_SOCKET, SO_ERROR) = ok',
    'shutdown(SHUT_WR) = ok',
    'close() = ok',
    'rt_sigaction(SIGPIPE, {sa_handler=SIG_IGN, sa_mask=[], sa_flags=SA_RESTORER, sa_restorer=ADDR}) = ok',
    'clone(flags=CLONE_CHILD_CLEARTID|CLONE_CHILD_SETTID|SIGCHLD) = ok',
    'clone3(flags=CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|CLONE_SYSVSEM|CLONE_SETTLS'
    '|CLONE_PARENT_SETTID|CLONE_CHILD_CLEARTID) = ok',
    'wait4(WNOHANG) = ok',
    'signal SIGCHLD',
    'epoll_wait() = ERESTARTSYS',
    'epoll_wait() = ?',
    '+++ killed by SIGTERM +++',
]


class Signatures(unittest.TestCase):
    def test_each_line_keeps_behaviour_and_drops_fds_addresses_ports_and_lengths(self):
        self.assertEqual([signature(line) for line in TRACE.splitlines()], EXPECTED)

    def test_blank_lines_carry_nothing_and_unknown_lines_fail(self):
        self.assertIsNone(signature('  '))
        with self.assertRaises(ValueError):
            signature('strace: Process 4242 attached')

    def test_epoll_events_survive_their_data(self):
        self.assertEqual(signature('epoll_ctl(4, EPOLL_CTL_MOD, 5, {events=EPOLLIN|EPOLLOUT|EPOLLRDHUP, '
                                   'data={u32=5, u64=5}}) = 0'),
                         'epoll_ctl(EPOLL_CTL_MOD, {events=EPOLLIN|EPOLLOUT|EPOLLRDHUP}) = ok')

    def test_unix_paths_are_masked(self):
        self.assertEqual(signature('bind(3, {sa_family=AF_UNIX, sun_path="/tmp/swoole-corpus-x1/s.sock"}, 110) = 0'),
                         'bind({sa_family=AF_UNIX, sun_path=PATH}) = ok')

    def test_files_of_every_process_are_counted_together(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, 'trace.100').write_text('close(3) = 0\nclose(4) = 0\n')
            Path(directory, 'trace.101').write_text('close(3) = -1 EBADF (Bad file descriptor)\n+++ exited with 0 +++\n')
            Path(directory, 'other').write_text('not a trace')
            self.assertEqual(signatures(directory), Counter({'close() = ok': 2, 'close() = EBADF': 1,
                                                             '+++ exited with 0 +++': 1}))


class Differences(unittest.TestCase):
    def test_only_what_every_run_of_one_engine_made_and_no_run_of_the_other(self):
        runs = {'upstream': [Counter({'a': 1, 'b': 3, 'c': 1}), Counter({'a': 2, 'b': 1})],
                'cargo': [Counter({'a': 1, 'd': 1, 'e': 1}), Counter({'a': 1, 'd': 4, 'b': 1})]}
        only, unstable = differences(runs)
        # b: every upstream run, but one cargo run too. c: one upstream run. e: one cargo run.
        self.assertEqual(only, {'upstream': [], 'cargo': ['d']})
        self.assertEqual(unstable, ['b', 'c', 'e'])

    def test_allowlist_matches_engine_project_and_pattern(self):
        allowed = [{'engine': 'cargo', 'project': '0[12]-*', 'signature': 'prctl(PR_SET_NAME*', 'reason': 'thread names'}]
        self.assertEqual(explain('01-hello', 'cargo', 'prctl(PR_SET_NAME, ""...) = ok', allowed), allowed[0])
        self.assertIsNone(explain('01-hello', 'upstream', 'prctl(PR_SET_NAME, ""...) = ok', allowed))
        self.assertIsNone(explain('03-notes', 'cargo', 'prctl(PR_SET_NAME, ""...) = ok', allowed))
        self.assertEqual(explain('03-notes', 'cargo', 'x', [{'engine': 'cargo', 'signature': '*', 'reason': 'r'}])['reason'], 'r')


class Run(unittest.TestCase):
    def test_traced_engine_wraps_php_and_traces_only_the_listed_calls(self):
        command = syscalls.Traced('cargo', Path('/x/swoole.so'), 'php', (), Path('/t/trace')).command('server.php')
        self.assertEqual(command[:2], ['strace', '-f'])
        self.assertEqual(command[command.index('-o') + 1], '/t/trace')
        self.assertEqual(command[command.index('--') + 1:], ['php', '-n', '-d', 'extension=/x/swoole.so', 'server.php'])
        traced = [name.removeprefix('?') for name in command[command.index('-e') + 1].removeprefix('trace=').split(',')]
        self.assertIn('accept4', traced)
        self.assertNotIn('openat', traced)

    def test_main_fails_on_an_unexplained_difference_and_reports_every_one(self):
        def run_project(engine, project):
            extra = 'prctl(PR_SET_NAME, ""...) = 0\n' if engine.name == 'cargo' else 'listen(3, 128) = 0\n'
            extra += 'close(3) = 0\n' * (10 if engine.name == 'cargo' else 1)
            engine.trace.with_name('trace.1').write_text('listen(3, 512) = 0\n' * (project['id'] == '01-hello') + extra)
        manifest = [{'id': '01-hello'}, {'id': '11-driver', 'mode': 'driver'}]
        with tempfile.TemporaryDirectory() as directory:
            allowlist, output = Path(directory, 'allow.json'), Path(directory, 'out.json')
            allowlist.write_text(json.dumps([{'engine': 'cargo', 'signature': 'prctl(PR_SET_NAME*', 'reason': 'names'},
                                             {'engine': 'cargo', 'signature': 'never', 'reason': 'stale'}]))
            argv = ['syscalls.py', '--engine', 'cargo=/c.so', '--engine', 'upstream=/u.so', '--allowlist', str(allowlist),
                    '--output', str(output)]
            with mock.patch.object(sys, 'argv', argv), mock.patch('corpus.run.MANIFEST', manifest), \
                    mock.patch('corpus.run.run_project', run_project), mock.patch('shutil.which', return_value='/usr/bin/strace'), \
                    mock.patch('tools.harness.Engine.metadata', lambda engine: {'name': engine.name}), \
                    redirect_stdout(io.StringIO()) as out:
                self.assertEqual(syscalls.main(), 1)
            report = json.loads(output.read_text())
        self.assertIn('01-hello             only upstream: listen(128) = ok', out.getvalue())
        [row] = report['projects']
        self.assertEqual(row['differences'], [
            {'engine': 'cargo', 'signature': 'prctl(PR_SET_NAME, ""...) = ok', 'reason': 'names'},
            {'engine': 'upstream', 'signature': 'listen(128) = ok', 'reason': None}])
        self.assertEqual(row['counts']['cargo'], {'listen(512) = ok': 2, 'prctl(PR_SET_NAME, ""...) = ok': 2, 'close() = ok': 20})
        # Twenty closes against two is listed; two listens against none is within two calls a run.
        self.assertEqual(row['count_differences'], {'close() = ok': [20, 2]})
        self.assertIn('01-hello             count cargo 20, upstream 2: close() = ok', out.getvalue())
        self.assertEqual(report['stale'], [{'engine': 'cargo', 'signature': 'never', 'reason': 'stale'}])


if __name__ == '__main__':
    unittest.main()
