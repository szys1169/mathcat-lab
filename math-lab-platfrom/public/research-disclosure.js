const SCOPES = new Set(["family", "route", "node"]);

function key(value) {
  return String(value ?? "");
}

export function createResearchDisclosureStore() {
  const boards = new Map();

  function scopeFor(boardId, scope) {
    if (!SCOPES.has(scope)) throw new Error(`Unknown research disclosure scope: ${scope}`);
    const boardKey = key(boardId);
    if (!boards.has(boardKey)) {
      boards.set(boardKey, { family: new Set(), route: new Set(), node: new Set() });
    }
    return boards.get(boardKey)[scope];
  }

  return {
    isCollapsed(boardId, scope, itemId) {
      return scopeFor(boardId, scope).has(key(itemId));
    },
    toggle(boardId, scope, itemId) {
      const items = scopeFor(boardId, scope);
      const itemKey = key(itemId);
      if (items.has(itemKey)) {
        items.delete(itemKey);
        return false;
      }
      items.add(itemKey);
      return true;
    }
  };
}
