// Fixed-height virtual list over the window scroll. One row may be "expanded" (a detail panel
// below it) and adds `extra` pixels.

const topOf = (i, rowH, expanded) => i * rowH + (expanded && i > expanded.index ? expanded.extra : 0);

function rowAt(y, rowH, expanded) {
  if (expanded) {
    const after = (expanded.index + 1) * rowH;
    if (y >= after) return y < after + expanded.extra ? expanded.index : Math.floor((y - expanded.extra) / rowH);
  }
  return Math.floor(y / rowH);
}

export function computeRange({ scrollTop, viewport, rowH, count, overscan = 6, expanded = null }) {
  if (count <= 0) return { start: 0, end: 0, padTop: 0, padBottom: 0 };
  const ex = expanded && expanded.index >= 0 && expanded.index < count ? expanded : null;
  const total = topOf(count, rowH, ex);
  const y0 = Math.max(0, scrollTop), y1 = Math.max(0, scrollTop + viewport);
  const start = Math.min(count, Math.max(0, rowAt(y0, rowH, ex) - overscan));
  const end = Math.min(count, Math.max(start, rowAt(y1, rowH, ex) + 1 + overscan));
  const padTop = topOf(start, rowH, ex);
  return { start, end, padTop, padBottom: total - topOf(end, rowH, ex) };
}

const OVERSCAN = 6;

/**
 * Renders rows `render(i) -> html string` into `container`, which scrolls with the window.
 * The expanded row's real height is measured after rendering, so `expanded.extra` is a hint.
 */
export function mountVList(container, opts) {
  let o = { ...opts };
  let cur = { start: -1, end: -1 };
  let nodes = new Map();
  const top = document.createElement('div');
  const bottom = document.createElement('div');
  top.className = bottom.className = 'vpad';
  container.replaceChildren(top, bottom);

  const origin = () => {
    const r = container.getBoundingClientRect();
    const cs = getComputedStyle(container);
    return r.top + container.clientTop + parseFloat(cs.paddingTop || 0);
  };

  function make(i) {
    const tpl = document.createElement('template');
    tpl.innerHTML = o.render(i).trim();
    const el = tpl.content.firstElementChild;
    el.dataset.vi = i;
    return el;
  }

  function layout(force) {
    const r = computeRange({ scrollTop: -origin(), viewport: innerHeight, rowH: o.rowH, count: o.count, overscan: OVERSCAN, expanded: o.expanded });
    if (!force && r.start === cur.start && r.end === cur.end) return;
    const next = new Map();
    for (let i = r.start; i < r.end; i++) next.set(i, (!force && nodes.get(i)) || make(i));
    for (const [i, el] of nodes) if (next.get(i) !== el) el.remove();
    let prev = top;
    for (let i = r.start; i < r.end; i++) {
      const el = next.get(i);
      if (prev.nextSibling !== el) prev.after(el);
      prev = el;
    }
    nodes = next;
    cur = r;
    top.style.height = r.padTop + 'px';
    bottom.style.height = r.padBottom + 'px';
    const ex = o.expanded && nodes.get(o.expanded.index);
    if (ex) {
      const extra = Math.round(ex.getBoundingClientRect().height) - o.rowH;
      if (extra > 0 && extra !== o.expanded.extra) {
        o.expanded = { ...o.expanded, extra };
        const rr = computeRange({ scrollTop: -origin(), viewport: innerHeight, rowH: o.rowH, count: o.count, overscan: OVERSCAN, expanded: o.expanded });
        bottom.style.height = rr.padBottom + 'px';
      }
    }
  }

  let raf = 0;
  const onScroll = () => {
    if (!raf) raf = requestAnimationFrame(() => { raf = 0; if (container.isConnected) layout(false); else destroy(); });
  };
  function destroy() {
    removeEventListener('scroll', onScroll);
    removeEventListener('resize', onScroll);
  }
  addEventListener('scroll', onScroll, { passive: true });
  addEventListener('resize', onScroll, { passive: true });
  layout(true);

  return {
    /** New options (count, rowH, render, expanded); always re-renders the visible rows. */
    update(next) {
      o = { ...o, ...next };
      layout(true);
    },
    /**
     * Window-relative scroll so row `i` is visible: align 'center' | 'start' | 'nearest', or
     * 'reveal' (centre it, but only when it is not already fully visible).
     */
    scrollToIndex(i, align = 'nearest', behavior = 'auto') {
      if (i < 0 || i >= o.count) return;
      const y = origin() + topOf(i, o.rowH, o.expanded);
      const h = o.rowH;
      const head = (document.querySelector('.topbar') || { getBoundingClientRect: () => ({ bottom: 0 }) }).getBoundingClientRect().bottom;
      const player = document.getElementById('player');
      const foot = innerHeight - (player && !player.hidden ? player.offsetHeight : 0);
      let delta;
      if (align === 'reveal') align = y < head || y + h > foot ? 'center' : 'none';
      if (align === 'none') return;
      if (align === 'start') delta = y - head;
      else if (align === 'center') delta = y - (head + foot) / 2 + h / 2;
      else if (y < head) delta = y - head;
      else if (y + h > foot) delta = y + h - foot;
      else return;
      scrollBy({ top: delta, behavior });
    },
    /** The rendered element for row `i`, if it is in the current window. */
    node(i) {
      return nodes.get(i) || null;
    },
    destroy,
  };
}
