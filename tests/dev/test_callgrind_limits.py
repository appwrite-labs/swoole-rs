"""benchmarks/callgrind.py's ratchet: a recorded ratio may drift by noise, no more."""
from pathlib import Path
import socket
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from benchmarks import callgrind  # noqa: E402


class Ratchet(unittest.TestCase):
    def test_within_noise_passes(self):
        self.assertEqual(callgrind.verdict(1.205, recorded=1.20)[1:], (False, False))

    def test_above_noise_fails(self):
        self.assertTrue(callgrind.verdict(1.23, recorded=1.20)[1])

    def test_beating_the_record_asks_to_lower_it(self):
        self.assertEqual(callgrind.verdict(1.17, recorded=1.20)[1:], (False, True))

    def test_recorded_ratio_overrides_max_ratio(self):
        self.assertFalse(callgrind.verdict(1.10, recorded=1.10, max_ratio=1.05)[1])
        self.assertTrue(callgrind.verdict(1.10, max_ratio=1.05)[1])

    def test_ratchet_lowers_to_what_every_result_beats_rounded_up(self):
        def result(**ratios):
            return {'instructions_per_request': {w: {'cargo': r * 1000, 'upstream': 1000} for w, r in ratios.items()}}
        results = [result(plaintext=0.951, json=0.99, timer=0.70), result(plaintext=0.962, json=1.0, websocket=0.5)]
        limits = {'plaintext': 1.00, 'json': 1.00, 'timer': 0.72, 'websocket': 0.87}
        # json beats nothing once rounded up; timer and websocket lack a result in one run.
        self.assertEqual(callgrind.ratchet(results, limits), {'plaintext': 0.97})
        self.assertEqual(callgrind.lowest(0.94), 0.94)

    def test_ratchet_rewrites_only_the_lowered_workload_files(self):
        from unittest import mock
        import json
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            limits = Path(temp, 'limits')
            limits.mkdir()
            (limits / 'plaintext').write_text('1.00\n')
            (limits / 'json').write_text('1.00\n')
            run = {'instructions_per_request': {'plaintext': {'cargo': 951, 'upstream': 1000},
                                                'json': {'cargo': 999, 'upstream': 1000}}}
            Path(temp, 'r.json').write_text(json.dumps(run))
            with mock.patch('sys.argv', ['callgrind.py', '--limits', str(limits), '--ratchet', f'{temp}/r.json']):
                self.assertEqual(callgrind.main(), 0)
            self.assertEqual((limits / 'plaintext').read_text(), '0.96\n')
            self.assertEqual((limits / 'json').read_text(), '1.00\n')
            self.assertEqual(callgrind.read_limits(limits), {'plaintext': 0.96, 'json': 1.00})

    def test_resolver_answers_the_question_and_drops_the_edns_record(self):
        port = int(callgrind.peers()['PEER_DNS_PORT'])
        question = b'\x09callgrind\x04test\x00\x00\x01\x00\x01'
        edns = b'\x00\x00\x29\x04\xd0\x00\x00\x00\x00\x00\x00'
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(5)
            sock.sendto(b'\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x01' + question + edns, ('127.0.0.1', port))
            reply = sock.recv(512)
        self.assertEqual(reply[:12], b'\x12\x34\x81\x80\x00\x01\x00\x01\x00\x00\x00\x00')
        self.assertEqual(reply[12:12 + len(question)], question)
        self.assertEqual(reply[-4:], bytes([127, 0, 0, 1]))

    def test_a_workload_over_its_limit_is_annotated(self):
        from unittest import mock
        import tempfile
        counts = {'cargo': 1100, 'upstream': 1000}
        with tempfile.TemporaryDirectory() as temp, \
                mock.patch.object(callgrind, 'instructions', side_effect=lambda engine, *_: counts[engine.name]), \
                mock.patch.object(callgrind, 'annotate') as annotate, \
                mock.patch('sys.argv', ['callgrind.py', '--engine', 'cargo=/x', '--engine', 'upstream=/y',
                                        '--max-ratio', '1.05', '--output', f'{temp}/r.json']):
            self.assertEqual(callgrind.main(), 1)
        annotate.assert_called_once_with('error', 'callgrind plaintext', 'ratio 1.1000 over limit 1.0500')

    def test_every_merge_tier_workload_is_recorded(self):
        limits = callgrind.read_limits(ROOT / 'benchmarks/callgrind_limits')
        workflow = ''.join((ROOT / '.github/workflows' / name).read_text() for name in ('callgrind.yml', 'variant.yml'))
        gated = {w for w in callgrind.WORKLOADS if f'--workload {w} ' in workflow or f'--workload {w}\n' in workflow}
        self.assertEqual(gated, set(limits))


if __name__ == '__main__':
    unittest.main()
