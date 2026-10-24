export interface Rect { x: number; y: number; w: number; h: number }
export interface Item { size: number }

/** Squarified treemap (Bruls, Huizing, van Wijk). Items must be sorted by size desc. */
export function squarify<T extends Item>(items: T[], box: Rect): { item: T; rect: Rect }[] {
  const out: { item: T; rect: Rect }[] = [];
  const total = items.reduce((s, i) => s + i.size, 0);
  if (total <= 0 || box.w <= 0 || box.h <= 0) return out;
  const scale = (box.w * box.h) / total;
  let rest = items.filter((i) => i.size > 0);
  let { x, y, w, h } = box;

  const worst = (row: T[], side: number) => {
    const sum = row.reduce((s, i) => s + i.size * scale, 0);
    let max = 0, min = Infinity;
    for (const i of row) { const a = i.size * scale; max = Math.max(max, a); min = Math.min(min, a); }
    return Math.max((side * side * max) / (sum * sum), (sum * sum) / (side * side * min));
  };

  while (rest.length) {
    const side = Math.min(w, h);
    let row: T[] = [rest[0]];
    let n = 1;
    while (n < rest.length && worst([...row, rest[n]], side) <= worst(row, side)) { row.push(rest[n]); n++; }
    rest = rest.slice(n);
    const sum = row.reduce((s, i) => s + i.size * scale, 0);
    if (w >= h) {
      const rw = sum / h; let cy = y;
      for (const i of row) { const rh = (i.size * scale) / rw; out.push({ item: i, rect: { x, y: cy, w: rw, h: rh } }); cy += rh; }
      x += rw; w -= rw;
    } else {
      const rh = sum / w; let cx = x;
      for (const i of row) { const rw = (i.size * scale) / rh; out.push({ item: i, rect: { x: cx, y, w: rw, h: rh } }); cx += rw; }
      y += rh; h -= rh;
    }
  }
  return out;
}
