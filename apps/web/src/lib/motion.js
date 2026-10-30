// Motion shared by the surfaces that open and close: the side panes, the
// task list's folds and the menus. Everything follows two curves and two
// lengths, mirrored in app.css as --ease-open, --ease-close,
// --open and --close: opening starts at once and settles softly; closing is
// shorter and even. A menu takes the quick lengths. Each surface's contents
// move with it rather than arriving on their own. Reduce Motion skips all of it.
export const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

export const OPEN = 320, CLOSE = 260, QUICK_OPEN = 160, QUICK_CLOSE = 110;

// A CSS cubic-bezier as an easing function, so Svelte transitions run the
// same curves as the stylesheet.
function bezier(x1, y1, x2, y2) {
  const at = (a, b, t) => ((1 - 3 * b + 3 * a) * t + (3 * b - 6 * a)) * t * t + 3 * a * t;
  const slope = (a, b, t) => 3 * (1 - 3 * b + 3 * a) * t * t + 2 * (3 * b - 6 * a) * t + 3 * a;
  return (x) => {
    if (x <= 0 || x >= 1) return x <= 0 ? 0 : 1;
    let t = x;
    for (let i = 0; i < 6; i++) {
      const d = slope(x1, x2, t);
      if (Math.abs(d) < 1e-6) break;
      t -= (at(x1, x2, t) - x) / d;
    }
    // Newton's method can overshoot near flat spots; bisection settles it.
    if (Math.abs(at(x1, x2, t) - x) > 1e-4) {
      let lo = 0, hi = 1;
      t = x;
      for (let i = 0; i < 24; i++) { if (at(x1, x2, t) < x) lo = t; else hi = t; t = (lo + hi) / 2; }
    }
    return at(y1, y2, t);
  };
}
export const easeOpen = bezier(.35, .75, .25, 1);
export const easeClose = bezier(.4, 0, .2, 1);
export const EASE_OPEN = 'cubic-bezier(.35, .75, .25, 1)', EASE_CLOSE = 'cubic-bezier(.4, 0, .2, 1)';

// A side pane, as a drawer at the window's edge: an empty frame whose width
// is the room the week gives up, and the pane's body, kept at full width at
// the edge. Opening widens the frame from nothing while the body slides in
// beside it, fading up in the first part of the way; closing retraces it,
// shorter; turning back midway starts from where it is. The frame's width
// is layout, but the body moves by transform and opacity, which the
// compositor runs, so a long list is neither laid out nor painted again as
// it moves, and keeps moving if the page is busy. Shut, the frame keeps
// the body, inert and unrendered with its layout kept, so it opens again at
// once. side is -1 for a pane at the start of the window and 1 at the end;
// appear opens it from shut when it is made open; onshown(on) says when the
// body comes on screen and when it has left.
export function drawer(frame, params) {
  const body = frame.firstElementChild;
  let { open, side, onshown } = params, shown, anims = [];
  const settle = () => {
    for (const a of anims) a.cancel();
    anims = [];
    frame.classList.toggle('shut', !open);
    frame.inert = !open;
    if (shown !== open) onshown?.((shown = open));
  };
  function toggle(next) {
    if (next === open) return;
    const full = body.offsetWidth;
    const from = frame.classList.contains('shut') ? 0 : Math.min(1, frame.getBoundingClientRect().width / full);
    open = next;
    for (const a of anims) a.cancel();
    frame.classList.remove('shut');
    frame.inert = false;
    if (open && !shown) onshown?.((shown = true));
    const to = open ? 1 : 0;
    if (still() || from === to) { settle(); return; }
    const duration = (open ? OPEN : CLOSE) * Math.abs(to - from), ease = open ? easeOpen : easeClose;
    const timing = { duration, easing: open ? EASE_OPEN : EASE_CLOSE, fill: 'forwards' };
    const at = (p) => `translateX(${(1 - p) * side * 100}%)`;
    anims = [
      frame.animate({ width: [`${from * full}px`, `${to * full}px`] }, timing),
      body.animate({ transform: [at(from), at(to)] }, timing),
      body.animate({ opacity: Array.from({ length: 13 }, (_, i) => Math.min(1, 2.5 * (from + (to - from) * ease(i / 12)))) }, { duration, fill: 'forwards' }),
    ];
    anims[0].onfinish = settle;
  }
  if (params.appear && open) { open = false; settle(); toggle(true); } else settle();
  return {
    update(p) { onshown = p.onshown; toggle(p.open); },
    destroy() { for (const a of anims) a.cancel(); },
  };
}

// A drawer's height, opening and closing: the list below moves with its
// edge, and its contents fade in the first part of the way.
export function unfold(node) {
  const h = node.getBoundingClientRect().height;
  return ({ direction }) => {
    if (still()) return { duration: 0 };
    const open = direction === 'in';
    return {
      duration: open ? OPEN : CLOSE, easing: open ? easeOpen : easeClose,
      css: (t) => `height: ${t * h}px; overflow: hidden; opacity: ${Math.min(1, t * 2)}`,
    };
  };
}

// Counts and the arcs they describe: the Progress card's numbers and rings
// travel together on one length and curve, easing out without overshoot, so
// a number lands with its arc.
export const COUNT = 500, COUNT_EASE = 'cubic-bezier(.22, 1, .36, 1)';
export const easeCount = bezier(.22, 1, .36, 1);
