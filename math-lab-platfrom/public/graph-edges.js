// Keep structural branches separate from cross-references. Every relation remains
// in the model/inspector, even when its line is not shown in the overview.
export function graphEdgeId(edge) {
  return edge.id || `${edge.from}:${edge.relation || "requires"}:${edge.to}`;
}

export function routeGraphEdges(model, layout, { selectedId = null, showCrossLinks = false } = {}) {
  const proof = model.kind === "proof";
  const crossCounts = new Map();
  const edges = [];
  let crossLinkCount = 0;
  let hiddenCrossLinkCount = 0;
  let outerLane = 0;
  let width = layout.width;
  let height = layout.height;
  let right = 0;
  let bottom = 0;
  for (const p of layout.positions.values()) {
    right = Math.max(right, p.x + p.width);
    bottom = Math.max(bottom, p.y + p.height);
  }
  const ordered = [...model.edges].sort((a, b) => graphEdgeId(a).localeCompare(graphEdgeId(b)));
  const sameColumnCounts = new Map();
  const sameColumnSlots = new Map();
  if (!proof) for (const edge of ordered) {
    const from = layout.positions.get(edge.from), to = layout.positions.get(edge.to);
    if (from && to && from.x === to.x) sameColumnCounts.set(from.x, (sameColumnCounts.get(from.x) || 0) + 1);
  }
  for (const edge of ordered) {
    const from = layout.positions.get(edge.from);
    const to = layout.positions.get(edge.to);
    if (!from || !to) continue;
    const crossLink = proof && Boolean(layout.treeEdgeIds) && !layout.treeEdgeIds.has(graphEdgeId(edge));
    const selected = edge.from === selectedId || edge.to === selectedId;
    if (crossLink) {
      crossLinkCount += 1;
      for (const id of new Set([edge.from, edge.to])) crossCounts.set(id, (crossCounts.get(id) || 0) + 1);
      if (!showCrossLinks && !selected && edge.relation !== "contradicts") {
        hiddenCrossLinkCount += 1;
        continue;
      }
    }
    let points, labelX, labelY;
    if (proof) {
      const x1 = from.x + from.width / 2, y1 = from.y + from.height;
      const x2 = to.x + to.width / 2, y2 = to.y;
      if (!crossLink && y2 > y1) {
        const middle = (y1 + y2) / 2;
        points = [[x1, y1], [x1, middle], [x2, middle], [x2, y2]];
      } else {
        // The horizontal segments stay in row gaps, the long segment outside
        // the tree. This also handles ancestor references and self-loops.
        const lane = right + 30 + outerLane++ * 14;
        points = [[x1, y1], [x1, y1 + 18], [lane, y1 + 18], [lane, Math.max(4, y2 - 18)], [x2, Math.max(4, y2 - 18)], [x2, y2]];
        width = Math.max(width, lane + 36);
        height = Math.max(height, y1 + 48);
      }
      labelX = x2;
      labelY = y2 - 9;
    } else {
      const x1 = from.x + from.width, y1 = from.y + from.height / 2;
      const x2 = to.x, y2 = to.y + to.height / 2;
      // Adjacent DAG columns share a clear vertical channel. Long links route
      // below the graph instead of cutting through intervening cards.
      const gap = x2 - x1;
      if (gap > 0 && gap <= 150) {
        const middle = (x1 + x2) / 2;
        points = [[x1, y1], [middle, y1], [middle, y2], [x2, y2]];
        labelX = middle;
        labelY = (y1 + y2) / 2 - 6;
      } else if (from.x === to.x) {
        const slot = sameColumnSlots.get(from.x) || 0;
        sameColumnSlots.set(from.x, slot + 1);
        const lane = x1 + 18 + (slot + 0.5) * 50 / sameColumnCounts.get(from.x);
        const endY = edge.from === edge.to ? y2 + 24 : y2;
        points = [[x1, y1], [lane, y1], [lane, endY], [to.x + to.width, endY]];
        labelX = lane + 4;
        labelY = (y1 + endY) / 2 - 6;
        width = Math.max(width, lane + 36);
      } else {
        const lane = bottom + 30 + outerLane++ * 14;
        points = [[x1, y1], [x1 + 22, y1], [x1 + 22, lane], [x2 - 22, lane], [x2 - 22, y2], [x2, y2]];
        labelX = (x1 + x2) / 2;
        labelY = lane - 6;
        height = Math.max(height, lane + 36);
      }
    }
    edges.push({ ...edge, crossLink, selected, points,
      path: points.map(([x, y], index) => `${index ? "L" : "M"}${x} ${y}`).join(" "),
      labelX, labelY, showLabel: selected && !crossLink });
  }
  return { edges, width, height, crossCounts, crossLinkCount, hiddenCrossLinkCount };
}
