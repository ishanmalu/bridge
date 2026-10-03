(() => {
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const $ = (s) => document.querySelector(s);
  const $$ = (s) => [...document.querySelectorAll(s)];

  // Title letters rise in one by one.
  const title = $('#title');
  if (title && !reduce) {
    const text = title.textContent;
    title.textContent = '';
    [...text].forEach((c, i) => {
      const s = document.createElement('span');
      s.className = 'ch';
      s.textContent = c;
      s.setAttribute('aria-hidden', 'true');
      s.style.setProperty('--i', i);
      title.appendChild(s);
    });
  }

  // Top bar gets a hairline once you scroll.
  const top = $('.top');
  addEventListener('scroll', () => top.classList.toggle('scrolled', scrollY > 8), { passive: true });

  // Reveal sections as they arrive, staggering siblings.
  $$('.grid, .flow, .faq').forEach((g) => g.querySelectorAll('.reveal').forEach((el, i) => el.style.setProperty('--d', i % 4)));
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add('in');
      if (e.target.querySelector('#code')) e.target.querySelector('#code').classList.add('go');
      io.unobserve(e.target);
    }
  }, { threshold: 0.18 });
  $$('.reveal').forEach((el) => (reduce ? el.classList.add('in') : io.observe(el)));
  $$('#code b').forEach((b, i) => b.style.setProperty('--i', i));

  // Modifier pairs light up in turn while visible.
  const pairs = $$('.pair');
  let pi = 0, pairTimer = null;
  const keymapIO = new IntersectionObserver(([e]) => {
    clearInterval(pairTimer);
    if (!e.isIntersecting || reduce) return;
    pairTimer = setInterval(() => {
      pairs.forEach((p) => p.classList.remove('on'));
      void pairs[pi].offsetWidth;
      pairs[pi].classList.add('on');
      pi = (pi + 1) % pairs.length;
    }, 1100);
  });
  if (pairs.length) keymapIO.observe($('#pairs'));

  // The desk: the pointer wanders on the Mac, pushes through the edge, lands on the PC,
  // brings the clipboard, shows ⌘C → Ctrl+C, then heads home.
  const desk = $('#desk'), cur = $('#cursor'), clip = $('#clip'), keys = $('#keys'), ripple = $('#ripple');
  const mac = $('#mac'), pc = $('#pc'), steps = $$('#steps li'), beams = $$('#beam i');
  const macEdge = mac.querySelector('.edge'), pcEdge = pc.querySelector('.edge');
  if (!desk) return;
  if (reduce) {
    const r = desk.getBoundingClientRect(), p = pc.getBoundingClientRect();
    cur.style.setProperty('transform', `translate(${p.left - r.left + p.width * .4}px, ${p.top - r.top + p.height * .45}px)`);
    return;
  }

  const ease = (t) => (t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
  const lerp = (a, b, t) => a + (b - a) * t;
  const clamp01 = (t) => Math.max(0, Math.min(1, t));
  const PERIOD = 10;
  let rippled = -1, visible = true;
  new IntersectionObserver(([e]) => (visible = e.isIntersecting)).observe(desk);

  function frame(now) {
    requestAnimationFrame(frame);
    if (!visible) return;
    const d = desk.getBoundingClientRect(), m = mac.getBoundingClientRect(), p = pc.getBoundingClientRect();
    const M = { l: m.left - d.left, t: m.top - d.top, w: m.width, h: m.height };
    const P = { l: p.left - d.left, t: p.top - d.top, w: p.width, h: p.height };
    const yEdgeM = M.t + M.h * .5, yEdgeP = P.t + P.h * .5;
    const t = (now / 1000) % PERIOD;
    const cycle = Math.floor(now / 1000 / PERIOD);
    let x, y, step = 0, onPc = false, push = 0, clipA = 0, keyA = 0;

    if (t < 1.6) { // wander on the Mac
      const k = ease(t / 1.6); x = lerp(M.l + M.w * .25, M.l + M.w * .62, k); y = lerp(M.t + M.h * .3, yEdgeM, k);
    } else if (t < 2.5) { // push into the edge
      const k = ease((t - 1.6) / .9); x = lerp(M.l + M.w * .62, M.l + M.w - 8, k); y = yEdgeM; push = k;
    } else if (t < 4.8) { // arrive on the PC with the clipboard
      const k = ease((t - 2.5) / 2.3); x = lerp(P.l + 6, P.l + P.w * .46, k); y = lerp(yEdgeP, P.t + P.h * .3, k);
      onPc = true; step = 1; clipA = clamp01((t - 2.6) * 3) * clamp01((4.8 - t) * 3);
      if (rippled !== cycle) { rippled = cycle; ripple.style.setProperty('left', `${P.l + 6}px`); ripple.style.setProperty('top', `${yEdgeP}px`); ripple.classList.remove('go'); void ripple.offsetWidth; ripple.classList.add('go'); }
    } else if (t < 6.8) { // shortcuts work over there
      x = P.l + P.w * .46 + Math.sin((t - 4.8) * 3) * 4; y = P.t + P.h * .3; onPc = true; step = 2;
      keyA = clamp01((t - 4.9) * 3) * clamp01((6.8 - t) * 3);
    } else if (t < 8) { // head back to the edge
      const k = ease((t - 6.8) / 1.2); x = lerp(P.l + P.w * .46, P.l + 8, k); y = lerp(P.t + P.h * .3, yEdgeP, k);
      onPc = true; step = 3; push = k > .7 ? (k - .7) / .3 : 0;
    } else { // home again
      const k = ease((t - 8) / 2); x = lerp(M.l + M.w - 10, M.l + M.w * .25, k); y = lerp(yEdgeM, M.t + M.h * .3, k); step = 3;
    }

    cur.style.setProperty('transform', `translate(${x}px, ${y}px)`);
    // Chips sit centred low on the PC's screen, above the taskbar, so they never spill off it.
    const chipAt = (el, a) => {
      const sc = Math.min(1, (P.w - 12) / el.offsetWidth);
      const cw = el.offsetWidth * sc, ch = el.offsetHeight * sc;
      const cx = P.l + (P.w - cw) / 2, cy = P.t + P.h * .88 - ch;
      el.style.setProperty('transform', `translate(${cx}px, ${cy + (1 - a) * 8}px) scale(${sc})`);
    };
    chipAt(clip, clipA);
    clip.style.setProperty('opacity', clipA.toFixed(2));
    chipAt(keys, keyA);
    keys.style.setProperty('opacity', keyA.toFixed(2));
    const lit = push > .5;
    macEdge.classList.toggle('on', lit && !onPc);
    pcEdge.classList.toggle('on', lit && onPc);
    mac.classList.toggle('lit', !onPc && t < 8 && push > .8);
    pc.classList.toggle('lit', onPc && step < 3);
    steps.forEach((s, i) => s.classList.toggle('on', i === step));
    const crossing = (t > 2.2 && t < 2.9) || (t > 7.7 && t < 8.4);
    beams.forEach((b, i) => {
      const phase = t > 7 ? (t - 7.7) / .7 : (t - 2.2) / .7;
      const order = t > 7 ? beams.length - 1 - i : i;
      b.classList.toggle('on', crossing && Math.abs(phase * beams.length - order - .5) < .8);
    });
  }
  requestAnimationFrame(frame);
})();
