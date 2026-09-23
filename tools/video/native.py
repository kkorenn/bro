#!/usr/bin/env python3
"""Fast native regression tests without rebuilding Servo."""
from pathlib import Path
import os
import shlex
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
native = root / 'servo/components/media/backends/gstreamer/mse-sys/native'
with tempfile.TemporaryDirectory(prefix='bro-native-video-') as directory:
    work = Path(directory)
    video = work / 'video.mp4'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=24',
                    '-t', '12', '-c:v', 'libx264', '-profile:v', 'baseline', '-g', '24',
                    '-movflags', 'frag_keyframe+empty_moov+default_base_moof', str(video)], check=True)
    flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'gstreamer-app-1.0'], text=True))
    sdk = []
    if sys.platform == 'darwin':
        sdk = ['-isysroot', subprocess.check_output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], text=True).strip()]
    binary = work / 'native-test'
    subprocess.run([os.environ.get('CC', 'cc'), '-O1', *sdk, '-DBUILDING_GST_MSE', '-DGST_USE_UNSTABLE_API',
                    '-I'+str(native), str(root / 'tools/video/native.c'),
                    *map(str, sorted(native.rglob('*.c'))), *flags, '-o', str(binary)], check=True)
    subprocess.run([str(binary), str(video)], check=True, timeout=30)
