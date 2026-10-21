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
