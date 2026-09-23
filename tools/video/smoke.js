// Opt-in development probe. Runs once per document; never injected in release builds.
(() => {
  if (window.__broVideoSmoke || !document.querySelector('video')) return;
  window.__broVideoSmoke = true;
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  const assert = (ok, label) => { if (!ok) throw new Error(label); };
  const until = async (predicate, label, timeout = 20000) => {
    const deadline = Date.now() + timeout;
    while (!predicate()) {
      if (Date.now() > deadline) throw new Error('timeout: ' + label);
      await wait(100);
    }
  };
  (async () => {
    const v = document.querySelector('video');
    await until(() => v.readyState >= 2, 'loaded video');
    v.muted = true;
    await v.play();
    const start = v.currentTime;
    await until(() => v.currentTime > start + 3, 'sustained playback');
    v.pause();
    await wait(300);
    const paused = v.currentTime;
    await wait(500);
    assert(Math.abs(v.currentTime - paused) < 0.2, 'pause holds position');
    assert(v.videoWidth > 0 && v.videoHeight > 0, 'decoded dimensions');
    const target = Math.min(1, v.duration / 4);
    v.currentTime = target;
    await until(() => !v.seeking && Math.abs(v.currentTime - target) < 0.5, 'backward seek');
    v.playbackRate = 1.5;
    await v.play();
    await until(() => v.currentTime > target + 3, 'play after seek at 1.5x');
    v.playbackRate = 1;
    assert(!v.error, 'no media error');
    console.log('BRO_VIDEO_PASS ' + JSON.stringify({url: location.href, width: v.videoWidth,
      height: v.videoHeight, time: v.currentTime, duration: v.duration, source: v.currentSrc.split(':')[0]}));
  })().catch(error => console.error('BRO_VIDEO_FAIL ' + error.message));
})();
