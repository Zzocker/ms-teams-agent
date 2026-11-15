#!/usr/bin/env python3
"""Linux native-app read-only stress profile; no third-party Python packages."""
import argparse
import csv
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise ValueError(f'Profiling hook changed; expected exactly one occurrence of {old!r}')
    return text.replace(old, new, 1)


def prepare(destination, headless=False):
    for name in ('src', 'assets'):
        shutil.copytree(ROOT / name, destination / name)
    shutil.copy2(ROOT / 'Cargo.lock', destination / 'Cargo.lock')
    manifest = (ROOT / 'Cargo.toml').read_text()
    manifest = replace_once(manifest, '[package]\n', '[package]\nautobins = false\n')
    manifest += '\n[[bin]]\nname = "ms-teams-agent-profile"\npath = "src/main.rs"\n'
    (destination / 'Cargo.toml').write_text(manifest)
    path = destination / 'src/main.rs'
    source = replace_once(path.read_text(), '    changed || revealed\n}',
                          '    changed | revealed | profile_tick(s, tx)\n}')
    # Profiling never registers push endpoints, shows notifications, or keeps a hidden app alive.
    source = replace_once(source, '        notifications::start(s);\n', '')
    source = replace_once(source, '    let desktop = std::env::args().len() == 1;', '    let desktop = false;')
    source = replace_once(source,
                          'if std::env::args().len() == 1 && let Screen::Main { session, chats } = &s.screen {\n        let mine =',
                          'if false && let Screen::Main { session, chats } = &s.screen {\n        let mine =')
    if headless:
        # Only this profiling copy may use real sessions with Rune's headless arguments.
        guard = 'std::env::args().len() == 1'
        if source.count(guard) != 2:
            raise ValueError('Headless sign-in/media hooks changed; refusing to profile demo data')
        source = source.replace(guard, 'true')
    source = replace_once(source, '    if let Some((id, link, r)) = s.older_loader.take() {', '    if let Some((id, link, r)) = s.older_loader.take() {\n        if let Err(error) = &r { eprintln!(\"PROFILE_ERROR older: {error}\"); }')
    # Stop background media reloads once the cache-clear experiment begins.
    source = replace_once(source, 'if true && let Screen::Main { session, chats } = &s.screen {' if headless else 'if std::env::args().len() == 1 && let Screen::Main { session, chats } = &s.screen {',
                          'if !PROFILE_CLEARED.load(SeqCst) && let Screen::Main { session, chats } = &s.screen {')
    path.write_text(source + '\n' + (ROOT / 'scripts/profile_driver.rs').read_text()
                    + '\n' + (ROOT / 'scripts/profile_alloc.rs').read_text())
    # Fail closed if these handlers move; disable user-initiated writes in this copy too.
    for filename, signatures in {
        'src/actions.rs': ('pub(crate) fn sign_out(s: &mut State) {',
                           'pub(crate) fn send(s: &mut State) {',
                           'pub(crate) fn delete_message(s: &mut State) {'),
        'src/teams.rs': ('pub fn mark_read(s: &Session, chat_id: &str, page: &Page) -> Result<(), String> {',),
    }.items():
        path = destination / filename
        source = path.read_text()
        for signature in signatures:
            result = 'return Ok(());' if 'mark_read' in signature else 'return;'
            source = replace_once(source, signature, '#[allow(unreachable_code, unused_variables)]\n' + signature + '\n    ' + result)
        path.write_text(source)
    path = destination / 'src/images.rs'
    path.write_text(path.read_text() + '''
impl Images {
    pub(crate) fn profile_bytes(&self) -> u64 {
        self.images.values().map(|e| cost(&e.image)).sum()
    }

    pub(crate) fn profile_counts(&self) -> (usize, usize, usize) {
        let previews = self.images.iter().filter(|(url, e)| e.image.is_some() && !url.contains("/views/imgpsh_fullsize_anim")).count();
        let full = self.images.values().filter(|e| e.image.is_some()).count() - previews;
        (previews, full, self.images.len() - previews - full)
    }
}
''')
    path = destination / 'src/avatars.rs'
    path.write_text(path.read_text() + '''
impl Avatars {
    pub(crate) fn profile_bytes(&self) -> u64 {
        self.images.values().filter_map(|(i, _)| i.as_ref().and_then(Image::size))
            .map(|[w, h]| u64::from(w) * u64::from(h) * 4 * 4 / 3).sum()
    }
}
''')


def fields(text):
    return dict(line.split(':', 1) for line in text.splitlines() if ':' in line)


def export(log, output, exit_code, headless=False):
    lines = log.read_text().splitlines()
    rows = [json.loads(line[8:]) for line in lines if line.startswith('PROFILE ')]
    if not rows:
        raise ValueError(f'No samples collected; inspect {log}')
    first, last = rows[0], rows[-1]
    ticks = os.sysconf('SC_CLK_TCK')

    def stat(row):
        return row['stat'].rsplit(')', 1)[1].split()

    def cpu(row):
        s = stat(row)
        return (int(s[11]) + int(s[12])) / ticks

    def memory(row, key):
        return int(fields(row['smaps'])[key].split()[0]) / 1024

    duration = last['elapsed'] - first['elapsed']
    io_first, io_last = fields(first['io']), fields(last['io'])
    action_count = sum(line.startswith('PROFILE_ACTION') for line in lines)
    baseline = next((r for r in reversed(rows) if r['step'] == 0), first)
    def pass_rss(step):
        sample = next((r for r in reversed(rows) if r['step'] == step and r.get('phase', 'loaded') == 'loaded'), None)
        return memory(sample, 'Rss') if sample else None

    phases = {}
    for row in rows:
        phase = row.get('phase', 'loaded')
        phases[phase] = {
            'elapsed': row['elapsed'], 'rss_mib': memory(row, 'Rss'),
            'image_previews': row.get('image_previews'),
            'image_mib': row['image_bytes'] / 1024**2,
            'avatar_mib': row['avatar_bytes'] / 1024**2,
            'allocations': row.get('allocations'),
        }
    done = 'PROFILE_DONE' in lines and exit_code == 0
    errors = [line for line in lines if line.startswith('PROFILE_ERROR')]
    report = {
        'complete': done and not errors,
        'mode': 'headless' if headless else 'desktop',
        'headless_rendering': {'fps': 60, 'size': [1000, 680], 'scale': 1} if headless else None,
        'duration_seconds': round(duration, 2),
        'chat_opens': sum(line.startswith('PROFILE_ACTION open') for line in lines),
        'scroll_top_actions': sum(line.startswith('PROFILE_ACTION scroll') for line in lines),
        'rss_mib': {'initial': memory(first, 'Rss'), 'before_chats': memory(baseline, 'Rss'),
                    'pass1_end': pass_rss(action_count // 2) if done else None,
                    'pass2_end': pass_rss(action_count) if done else None,
                    'final': memory(last, 'Rss'),
                    'peak': max(memory(r, 'Rss') for r in rows)},
        'pss_mib_final': memory(last, 'Pss'),
        'max_loaded_messages': max(r['messages'] for r in rows),
        'max_message_text_html_bytes': max(r['message_text_bytes'] for r in rows),
        'phase_endpoints': phases,
        'image_cache_counts': {key: {'final': last.get(key),
                                     'peak': max((r[key] for r in rows if key in r), default=None)}
                               for key in ('image_previews', 'image_full_size', 'image_failed')},
        'estimated_image_cache_mib_final': last['image_bytes'] / 1024**2,
        'estimated_avatar_cache_mib_final': last['avatar_bytes'] / 1024**2,
        'cpu_seconds': round(cpu(last) - cpu(first), 2),
        'cpu_percent_one_core_average': round((cpu(last) - cpu(first)) / duration * 100, 2) if duration else None,
        'io_deltas': {k: int(io_last[k]) - int(io_first[k]) for k in io_first},
        'recorded_block_io_delay_ms': (int(stat(last)[39]) - int(stat(first)[39])) / ticks * 1000,
        'errors': errors,
        'warnings': [line for line in lines if line.startswith('rune:')],
        'limitations': [
            'Real debug app; a temporary copy drives existing chat/page/scroll handlers. Unread chats are skipped; sending, deleting, read receipts and sign-out are disabled.',
            'Local auth refresh and image/disk caches behave normally. Do not interact with the profiling window.',
            'I/O counters and CPU include profiler overhead: three proc-file reads and one log write per second; driver wakes four times/second.',
            'Recorded block-I/O delay is not per-call or network latency; zero may mean delay accounting is disabled.',
            'Image estimates exclude extra animation frames/GPU allocations. Text size excludes capacities, metadata and layout.',
            'Allocation counters cover Rust System allocations, not native-library or GPU allocations; glibc totals include native malloc users. No allocation stacks are recorded. Counters are sampled concurrently.',
            'Cache clearing affects memory only; background media polling is disabled during idle. malloc_trim is a diagnostic intervention, not normal app behavior.',
            'This short run cannot prove a leak or attribute all memory without allocation-stack profiling.',
            'Headless mode renders offscreen at fixed 60 fps with no window/compositor presentation. GPU backend and memory can differ; compare results within the same mode.',
        ],
    }
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    columns = ['elapsed', 'step', 'messages', 'message_text_bytes', 'rss_mib', 'pss_mib', 'image_bytes', 'avatar_bytes', 'image_previews', 'image_full_size', 'image_failed', 'phase', 'rust_live_bytes', 'rust_peak_bytes', 'rust_allocated_bytes', 'rust_freed_bytes', 'glibc_used_bytes', 'glibc_free_arena_bytes']
    with (output / 'samples.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=columns)
        writer.writeheader()
        for row in rows:
            writer.writerow({**{k: row[k] for k in columns if k in row},
                             **{k: v for k, v in row.get('allocations', {}).items() if k in columns},
                             'rss_mib': memory(row, 'Rss'), 'pss_mib': memory(row, 'Pss')})
    print(json.dumps(report, indent=2))
    print(f'\nResults: {output}')
    return report['complete']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--headless', action='store_true', help='Render offscreen with real Teams data at 60 fps; CPU and graphics memory differ from desktop mode')
    parser.add_argument('--output', type=Path, help='New directory for run.log, report.json and samples.csv (default: unique directory in /tmp)')
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('Linux /proc is required')
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix='teams-profile-'))
    if args.output:
        output.mkdir(parents=True, exist_ok=False)
    print(f'Results: {output}', flush=True)
    with tempfile.TemporaryDirectory(prefix='teams-profile-build-') as tmp:
        work = Path(tmp)
        prepare(work, args.headless)
        command = ['cargo', 'build', '--locked', '--manifest-path', str(work / 'Cargo.toml'),
                   '--bin', 'ms-teams-agent-profile', '--target-dir', str(ROOT / 'target')]
        subprocess.run(command, check=True, timeout=600)
        print(f'Running two passes through up to six already-read chats ({"headless" if args.headless else "desktop"}). Allow about four minutes including cache-clear idle phases.', flush=True)
        app_command = [str(ROOT / 'target/debug/ms-teams-agent-profile')]
        if args.headless:
            app_command += ['--duration', '340', '--realtime', '--fps', '60', '--scale', '1', '--size', '1000x680']
        else:
            print('Leave the profiling window alone.', flush=True)
        with (output / 'run.log').open('w') as log:
            child = subprocess.Popen(app_command, stdout=log, stderr=log, cwd=ROOT)
            try:
                code = child.wait(timeout=390)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                code = 1
        return 0 if export(output / 'run.log', output, code, args.headless) else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
