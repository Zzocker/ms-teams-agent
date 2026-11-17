"""Run with python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest

import profile as profiler


class ProfileTests(unittest.TestCase):
    def test_preparation_is_isolated_and_disables_writes(self):
        original = (profiler.ROOT / 'src/main.rs').read_bytes()
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            profiler.prepare(work)
            self.assertEqual((profiler.ROOT / 'src/main.rs').read_bytes(), original)
            actions = (work / 'src/actions.rs').read_text()
            for name in ('send', 'delete_message', 'sign_out'):
                self.assertIn(f'fn {name}(s: &mut State) {{\n    return;', actions)
            self.assertIn('horizon: &ReadHorizon) -> Result<(), String> {\n    return Ok(());',
                          (work / 'src/teams.rs').read_text())
            self.assertIn('remove: bool) -> Result<(), String> {\n    return Ok(());',
                          (work / 'src/teams/reactions.rs').read_text())
            self.assertIn('static PROFILE_ALLOCATOR', (work / 'src/main.rs').read_text())
            self.assertIn('!PROFILE_CLEARED.load(SeqCst)', (work / 'src/main.rs').read_text())
            self.assertIn('profile_tick(s, tx)', (work / 'src/main.rs').read_text())
            self.assertIn('std::env::args().len() == 1', (work / 'src/main.rs').read_text())
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            profiler.prepare(work, headless=True)
            main = (work / 'src/main.rs').read_text()
            self.assertNotIn('std::env::args().len() == 1', main)
            self.assertIn('sign_in(s, false);', main)
            self.assertIn('s.images.poll(session, urls', main)
        with self.assertRaises(ValueError):
            profiler.replace_once('a a', 'a', 'b')
        with self.assertRaises(ValueError):
            profiler.replace_once('a', 'missing', 'b')

    def test_counters_and_incomplete_runs(self):
        def sample(elapsed, cpu_ticks, rss, calls):
            stat = ['0'] * 40
            stat[11], stat[39] = str(cpu_ticks), str(cpu_ticks)
            return 'PROFILE ' + json.dumps({
                'elapsed': elapsed, 'step': 0, 'messages': 50, 'message_text_bytes': 12,
                'phase': 'loaded', 'allocations': {'rust_live_bytes': 2048},
                'image_previews': 3, 'image_full_size': 0, 'image_failed': 1,
                'image_bytes': 1024, 'avatar_bytes': 512,
                'smaps': f'Rss: {rss} kB\nPss: {rss} kB\n',
                'stat': '123 (name with ) brackets) ' + ' '.join(stat),
                'io': f'syscr: {calls}\nsyscw: 0\nread_bytes: 0\nwrite_bytes: 0\n',
            })
        ticks = os.sysconf('SC_CLK_TCK')
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            output = Path(directory)
            log = output / 'run.log'
            data = sample(0, 0, 1024, 5) + '\n' + sample(2, ticks, 2048, 8) + '\n'
            log.write_text(data + 'PROFILE_DONE\n')
            self.assertTrue(profiler.export(log, output, 0))
            report = json.loads((output / 'report.json').read_text())
            self.assertEqual(report['rss_mib']['peak'], 2)
            self.assertEqual(report['image_cache_counts']['image_previews'], {'final': 3, 'peak': 3})
            self.assertIn('image_previews', (output / 'samples.csv').read_text())
            self.assertEqual(report['cpu_percent_one_core_average'], 50)
            self.assertEqual(report['io_deltas']['syscr'], 3)
            self.assertEqual(report['recorded_block_io_delay_ms'], 1000)
            self.assertEqual(report['mode'], 'desktop')
            self.assertEqual(report['phase_endpoints']['loaded']['allocations']['rust_live_bytes'], 2048)
            cleared = json.loads(sample(32, ticks, 1024, 8)[8:])
            cleared.update(phase='caches_cleared', image_previews=0, image_bytes=0)
            log.write_text(data + 'PROFILE ' + json.dumps(cleared) + '\nPROFILE_DONE\n')
            self.assertTrue(profiler.export(log, output, 0))
            phases = json.loads((output / 'report.json').read_text())['phase_endpoints']
            self.assertEqual(phases['loaded']['rss_mib'], 2)
            self.assertEqual(phases['caches_cleared']['image_previews'], 0)
            self.assertEqual(phases['caches_cleared']['rss_mib'], 1)
            self.assertTrue(profiler.export(log, output, 0, headless=True))
            report = json.loads((output / 'report.json').read_text())
            self.assertEqual(report['mode'], 'headless')
            self.assertEqual(report['headless_rendering']['fps'], 60)
            log.write_text(data)
            self.assertFalse(profiler.export(log, output, 0))
            log.write_text(data + 'PROFILE_ERROR older: unavailable\nPROFILE_DONE\n')
            self.assertFalse(profiler.export(log, output, 0))
