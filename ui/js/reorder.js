// Drag to reorder fixed-height rows: a grip handle per row, an insertion line, auto-scroll near
// the edges. Works for virtual lists (rows outside the viewport are not rendered): the target
// index comes from the pointer position, the row height and the list's top, not from DOM nodes.

/**
 * Target index for a row dragged from `from` when the pointer is `offsetY` pixels below the top
 * of the list: a neighbour is only passed once the pointer is more than half way over it.
 * Clamped to 0..count-1.
 */
export function dropIndex(offsetY, rowH, count, from) {
  if (count <= 0) return 0;
  const raw = Math.floor(offsetY / rowH);
  const half = offsetY - raw * rowH;
  let to = raw;
  if (raw > from && half <= rowH / 2) to = raw - 1;
  else if (raw < from && half >= rowH / 2) to = raw + 1;
  return Math.max(0, Math.min(count - 1, to));
}

const EDGE = 40;
const val = (v) => (typeof v === 'function' ? v() : v);

/**
 * Makes rows of `container` draggable by their `handle` (a selector). Rows carry their index in
 * `data-vi` (virtual list) or `data-ri`. `rowH` and `count` may be functions.
 * `scroller`: window or the scrolling element. Optional: `bounds()` → { top, bottom } (client
 * coordinates of the visible part of the scroller, default the scroller itself), `onStart(from)`,
 * `onEnd()`, `rowOf(from)` → element to mark while dragging.
 * Alt+↑/↓ on a focused handle moves the row by one. Returns detach().
 */
export function attachReorder(container, { rowH, handle, count, onMove, scroller = window, bounds, onStart, onEnd, rowOf }) {
  let drag = null;
  const line = document.createElement('div');
  line.className = 'drop-line';
  line.hidden = true;

  const listTop = () => {
    const r = container.getBoundingClientRect();
    return r.top + container.clientTop + parseFloat(getComputedStyle(container).paddingTop || 0);
  };
  const view = () => {
    if (bounds) return bounds();
    if (scroller === window) return { top: 0, bottom: innerHeight };
    const r = scroller.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom };
  };
  const indexOf = (el) => {
    const row = el.closest('[data-vi],[data-ri]');
    if (!row || !container.contains(row)) return -1;
    const i = +(row.dataset.vi ?? row.dataset.ri);
    return Number.isInteger(i) ? i : -1;
  };
  const mark = (on) => {
    if (!drag) return;
    container.querySelectorAll('.dragging').forEach((el) => el.classList.remove('dragging'));
    if (!on) return;
    const el = rowOf ? rowOf(drag.from) : container.querySelector(`[data-vi="${drag.from}"],[data-ri="${drag.from}"]`);
    if (el) el.classList.add('dragging');
  };

  function update() {
    if (!drag) return;
    const h = val(rowH), n = val(count);
    drag.to = dropIndex(drag.y - listTop(), h, n, drag.from);
    if (drag.to === drag.from) line.hidden = true;
    else {
      const pad = parseFloat(getComputedStyle(container).paddingTop || 0);
      line.style.top = pad + (drag.to > drag.from ? drag.to + 1 : drag.to) * h - 1 + 'px';
      line.hidden = false;
    }
    mark(true);
  }

  function tick() {
    if (!drag) return;
    drag.raf = requestAnimationFrame(tick);
    const { top, bottom } = view();
    let d = 0;
    if (drag.y < top + EDGE) d = -Math.ceil(((top + EDGE - drag.y) / EDGE) * 14);
    else if (drag.y > bottom - EDGE) d = Math.ceil(((drag.y - (bottom - EDGE)) / EDGE) * 14);
    if (!d) return;
    if (scroller === window) scrollBy(0, d);
    else scroller.scrollTop += d;
    update();
  }

  function end(commit) {
    if (!drag) return;
    const { from, to, raf } = drag;
    cancelAnimationFrame(raf);
    mark(false);
    drag = null;
    line.hidden = true;
    container.classList.remove('reordering');
    document.removeEventListener('pointermove', onPointerMove, true);
    document.removeEventListener('pointerup', onPointerUp, true);
    document.removeEventListener('pointercancel', onPointerCancel, true);
    document.removeEventListener('keydown', onKey, true);
    if (onEnd) onEnd();
    if (commit && to !== from) onMove(from, to);
  }

  function onPointerMove(e) { if (drag) { drag.y = e.clientY; update(); } }
  function onPointerUp() { end(true); }
  function onPointerCancel() { end(false); }
  function onKey(e) { if (e.key === 'Escape') { e.preventDefault(); e.stopImmediatePropagation(); end(false); } }

  function onDown(e) {
    const h = e.target.closest(handle);
    if (!h || !container.contains(h) || e.button !== 0 || drag) return;
    const from = indexOf(h);
    if (from < 0) return;
    e.preventDefault();
    e.stopPropagation();
    drag = { from, to: from, y: e.clientY, raf: 0 };
    if (onStart) onStart(from);
    if (!line.isConnected) container.append(line);
    container.classList.add('reordering');
    // listen on the document: the row (and its handle) may be re-rendered during the drag
    document.addEventListener('pointermove', onPointerMove, true);
    document.addEventListener('pointerup', onPointerUp, true);
    document.addEventListener('pointercancel', onPointerCancel, true);
    document.addEventListener('keydown', onKey, true);
    update();
    drag.raf = requestAnimationFrame(tick);
  }

  function onKeyDown(e) {
    if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
    const h = e.target.closest(handle);
    if (!h || !container.contains(h)) return;
    const from = indexOf(h), to = from + (e.key === 'ArrowUp' ? -1 : 1);
    e.preventDefault();
    if (from >= 0 && to >= 0 && to < val(count)) onMove(from, to);
  }

  container.addEventListener('pointerdown', onDown);
  container.addEventListener('keydown', onKeyDown);
  return function detach() {
    end(false);
    container.removeEventListener('pointerdown', onDown);
    container.removeEventListener('keydown', onKeyDown);
    line.remove();
  };
}
