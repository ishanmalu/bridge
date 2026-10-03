// The pointer drifts on the Mac, pushes through its right edge and carries on over on the PC,
// bringing the clipboard with it; then it comes back the other way.
(() => {
  const stage = document.querySelector('.stage');
  const cur = document.getElementById('cursor');
  const clip = document.getElementById('clip');
  const [mac, pc] = document.querySelectorAll('.screen');
  if (!stage || matchMedia('(prefers-reduced-motion: reduce)').matches) {
    if (cur) cur.style.setProperty('transform', 'translate(45%, 40%)');
    return;
  }
  const ease = (t) => t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
  const lerp = (a, b, t) => a + (b - a) * t;
  const PERIOD = 7;

  function frame(now) {
    const W = stage.clientWidth, H = stage.clientHeight;
    const m = mac.getBoundingClientRect(), p = pc.getBoundingClientRect(), s = stage.getBoundingClientRect();
    const macR = m.right - s.left - 6, pcL = p.left - s.left + 6, y0 = H * .55;
    const t = (now / 1000) % PERIOD;
    let x, y, onPc, showClip = 0;
    if (t < 1.4) { // wander on the Mac
      const k = ease(t / 1.4); x = lerp(W * .12, W * .3, k); y = lerp(H * .3, y0, k);
    } else if (t < 2.2) { // push into the edge
      const k = ease((t - 1.4) / .8); x = lerp(W * .3, macR, k); y = y0;
    } else if (t < 4.4) { // arrive on the PC at the same height
      const k = ease((t - 2.2) / 2.2); x = lerp(pcL, W * .78, k); y = lerp(y0, H * .35, k); onPc = true;
      showClip = Math.min(1, (t - 2.2) * 3) * (t < 3.8 ? 1 : Math.max(0, 1 - (t - 3.8) * 2));
    } else if (t < 5.4) { // head back
      const k = ease((t - 4.4) / 1); x = lerp(W * .78, pcL, k); y = lerp(H * .35, y0, k); onPc = true;
    } else { // and home
      const k = ease((t - 5.4) / 1.6); x = lerp(macR, W * .12, k); y = lerp(y0, H * .3, k);
    }
    cur.style.setProperty('transform', `translate(${x}px, ${y}px)`);
    clip.style.setProperty('transform', `translate(${x + 22}px, ${y + 30}px)`);
    clip.style.setProperty('opacity', showClip.toFixed(2));
    const pushing = t > 1.9 && t < 2.5;
    mac.classList.toggle('lit', pushing && !onPc);
    pc.classList.toggle('lit', pushing && !!onPc);
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
})();
