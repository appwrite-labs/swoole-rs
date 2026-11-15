"""Wall-clock counter summaries and bounded, response-validated WebSocket load."""
import unittest
from unittest.mock import Mock, patch
from benchmarks.wall import summary, websocket


def row(task_clock):
    return {'requests_per_second': 1e5, 'p50_ms': 0.25, 'p99_ms': 0.5, 'per_request': {'task-clock': task_clock}}


class Summary(unittest.TestCase):
    def test_one_uncounted_repetition_keeps_the_median_of_the_rest(self):
        result = summary([row(v) for v in (0.009, None, 0.008, 0.010, 0.012)])
        self.assertEqual(result['task-clock']['median'], 0.0095)
        self.assertEqual(result['task-clock']['samples'], 4)
        self.assertEqual(result['p50_ms']['samples'], 5)

    def test_a_counter_no_repetition_counted_is_absent(self):
        self.assertNotIn('task-clock', summary([row(None)] * 5))
        self.assertNotIn('cycles', summary([row(0.009)] * 5))


class WebSocketLoad(unittest.TestCase):
    def load(self, chunks, pipeline=2):
        client = Mock()
        client.socket.recv.side_effect = chunks
        self.client = client
        with patch('benchmarks.wall.WebSocket', return_value=client), \
                patch('benchmarks.wall.os.urandom', return_value=b'\0' * 4), \
                patch('benchmarks.wall.time.monotonic', side_effect=[0, 0, 2]), \
                patch('benchmarks.wall.time.perf_counter', side_effect=[10, 11, 12, 13]):
            return websocket(9501, b'ping', 1, pipeline)

    def test_partial_and_coalesced_echoes_count_complete_messages(self):
        self.assertEqual(self.load([b'\x81', b'\x04ping\x81', b'\x04ping']), (2, 3000, 3000))
        self.client.socket.sendall.assert_called_once_with((b'\x81\x84' + b'\0' * 4 + b'ping') * 2)
        self.assertEqual([call.args[0] for call in self.client.socket.recv.call_args_list], [12, 11, 5])
        self.client.close.assert_called_once()

    def test_serial_option_counts_one_echo(self):
        self.assertEqual(self.load([b'\x81\x04ping'], pipeline=1), (1, 1000, 1000))

    def test_eof_or_wrong_echo_fails_and_closes_the_connection(self):
        for chunks in ([b'\x81', b''], [b'\x81\x04pong']):
            with self.subTest(chunks=chunks), self.assertRaises(RuntimeError):
                self.load(chunks)
            self.client.close.assert_called_once()

    def test_nonpositive_pipeline_is_rejected_before_connecting(self):
        with patch('benchmarks.wall.WebSocket') as connect:
            with self.assertRaises(ValueError):
                websocket(9501, b'ping', 1, 0)
            connect.assert_not_called()


if __name__ == '__main__':
    unittest.main()
