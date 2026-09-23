#!/usr/bin/env python3
"""Build-independent video checks in bro itself. Requires ffmpeg and a graphical session."""
import argparse
import functools
import http.server
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('--url', help='Exercise playback, pause, seek, and speed on a live site')
parser.add_argument('--case', choices=['mp4','separate','webm','direct'])
parser.add_argument('--timeout', type=int, default=150)
parser.add_argument('--log', type=Path, default=Path('/tmp/bro-video-test.log'))
args = parser.parse_args()

with tempfile.TemporaryDirectory(prefix='bro-video-') as directory:
    server = None
    if args.url:
        url = args.url
    else:
        work = Path(directory)
        (work / 'index.html').write_text((ROOT / 'tools/video/fixture.html').read_text())
        base = ['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=24',
                '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000', '-t', '12']
        mp4 = ['-c:v', 'libx264', '-profile:v', 'baseline', '-g', '24', '-c:a', 'aac',
               '-movflags', 'frag_keyframe+empty_moov+default_base_moof']
        subprocess.run(base + mp4 + [str(work / 'av.mp4')], check=True)
        subprocess.run(base + ['-an'] + mp4 + [str(work / 'video.mp4')], check=True)
        subprocess.run(base + ['-vn'] + mp4 + [str(work / 'audio.mp4')], check=True)
        subprocess.run(base + ['-c:v', 'libvpx-vp9', '-g', '24', '-c:a', 'libopus', str(work / 'av.webm')], check=True)
        handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=directory)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        url = f'http://127.0.0.1:{server.server_port}/' + ('?case='+args.case if args.case else '')
    env = dict(os.environ)
    env.setdefault('RUST_LOG', 'warn')
    if args.url:
        env['BRO_VIDEO_SMOKE'] = '1'
    else:
        env.pop('BRO_VIDEO_SMOKE', None)
    passed = 'BRO_VIDEO_PASS' if args.url else 'BRO_FIXTURE_PASS'
    failed = 'BRO_VIDEO_FAIL' if args.url else 'BRO_FIXTURE_FAIL'
    with args.log.open('w') as output:
        process = subprocess.Popen([str(ROOT / 'target/debug/bro'), url], cwd=ROOT, env=env,
                                   stdout=output, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                text = args.log.read_text(errors='replace')
                if passed in text:
                    print(next(line for line in text.splitlines() if passed in line))
                    break
                if failed in text or process.poll() is not None:
                    raise SystemExit(f'FAIL; see {args.log}')
                time.sleep(.5)
            else:
                raise SystemExit(f'TIMEOUT; see {args.log}')
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            if server:
                server.shutdown()
