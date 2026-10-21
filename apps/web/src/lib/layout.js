// Bounds and overlap columns for the on-screen calendar.
export function layout(items) {
  const meetings = items.flatMap((it) => it.meetings);
  const startH = Math.min(8, ...meetings.map((m) => Math.floor(m.start / 60)));
  const endH = Math.max(17, ...meetings.map((m) => Math.ceil(m.end / 60)));
  return { days: 7, startH, endH, hours: endH - startH };
}

// Assign overlapping entries of one day to side-by-side columns. n is the
// column count of the cluster of mutually overlapping entries.
export function columns(entries) {
  const out = [];
  let cluster = [], ends = [], clusterEnd = 0;
  const flush = () => {
    for (const e of cluster) e.n = ends.length;
    out.push(...cluster);
    cluster = []; ends = []; clusterEnd = 0;
  };
  for (const src of [...entries].sort((a, b) => a.m.start - b.m.start)) {
    if (cluster.length && src.m.start >= clusterEnd) flush();
    const e = { ...src };
    e.col = ends.findIndex((end) => end <= e.m.start);
    if (e.col < 0) e.col = ends.push(0) - 1;
    ends[e.col] = e.m.end;
    clusterEnd = Math.max(clusterEnd, e.m.end);
    cluster.push(e);
  }
  flush();
  return out;
}

