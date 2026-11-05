// Multi-select over a song list (ids and indexes only, no DOM).
// `list` is the id[] in the current display order.

/** → { ids: Set, anchor: null|string } */
export function createSelection() {
  return { ids: new Set(), anchor: null };
}

/**
 * Click on `id`. Plain: select only it. Ctrl/Cmd: toggle it. Shift: the range from the anchor to it
 * (replacing the selection; with ctrl too, added to it). Ctrl and plain clicks move the anchor, shift
 * clicks keep it.
 */
export function clickSelect(sel, list, id, { ctrl = false, shift = false } = {}) {
  const from = sel.anchor == null ? -1 : list.indexOf(sel.anchor);
  const to = list.indexOf(id);
  if (shift && from >= 0 && to >= 0) {
    if (!ctrl) sel.ids.clear();
    const [lo, hi] = from < to ? [from, to] : [to, from];
    for (let i = lo; i <= hi; i++) sel.ids.add(list[i]);
    return sel;
  }
  if (ctrl) {
    if (sel.ids.has(id)) sel.ids.delete(id);
    else sel.ids.add(id);
  } else {
    sel.ids.clear();
    sel.ids.add(id);
  }
  sel.anchor = id;
  return sel;
}

export function selectAll(sel, list) {
  sel.ids = new Set(list);
  if (sel.anchor == null || !sel.ids.has(sel.anchor)) sel.anchor = list.length ? list[0] : null;
  return sel;
}

export function clearSelection(sel) {
  sel.ids.clear();
  sel.anchor = null;
  return sel;
}

/** Selected ids in list order. */
export function selectedInOrder(sel, list) {
  return list.filter((id) => sel.ids.has(id));
}

/** Drops ids no longer in `list` (after a page, search or filter change). */
export function pruneSelection(sel, list) {
  const keep = new Set(list);
  for (const id of [...sel.ids]) if (!keep.has(id)) sel.ids.delete(id);
  if (sel.anchor != null && !keep.has(sel.anchor)) sel.anchor = null;
  return sel;
}
