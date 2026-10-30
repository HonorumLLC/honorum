import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { easeOpen, easeClose, EASE_OPEN, EASE_CLOSE } from './motion.js';

// The curve a CSS cubic-bezier draws, found by bisection for reference.
function reference(css) {
  const [x1, y1, x2, y2] = css.match(/[\d.]+/g).map(Number);
  const at = (a, b, t) => 3 * a * t * (1 - t) ** 2 + 3 * b * t * t * (1 - t) + t ** 3;
  return (x) => {
    let lo = 0, hi = 1;
    for (let i = 0; i < 60; i++) { const t = (lo + hi) / 2; if (at(x1, x2, t) < x) lo = t; else hi = t; }
    return at(y1, y2, (lo + hi) / 2);
  };
}

test('the transition curves match the stylesheet curves', () => {
  const stylesheet = readFileSync(new URL('../app.css', import.meta.url), 'utf8');
  for (const [ease, name, string] of [[easeOpen, 'open', EASE_OPEN], [easeClose, 'close', EASE_CLOSE]]) {
    const css = stylesheet.match(new RegExp(`--ease-${name}:\\s*([^;]+);`))[1];
    assert.equal(string, css);
    const want = reference(css);
    assert.equal(ease(0), 0);
    assert.equal(ease(1), 1);
    let last = 0;
    for (let i = 1; i < 200; i++) {
      const x = i / 200, y = ease(x);
      assert.ok(Math.abs(y - want(x)) < 1e-4, `${css} at ${x}: ${y} vs ${want(x)}`);
      assert.ok(y >= last, `${css} turns back at ${x}`);
      last = y;
    }
  }
});
