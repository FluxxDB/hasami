// Undo/redo over immutable snapshots. The timeline model is pure, so a snapshot is just the
// Timeline object itself and undo is a pointer move.

export interface History<T> {
  past: T[];
  present: T;
  future: T[];
}

export const HISTORY_CAP = 200;

export function initHistory<T>(t: T): History<T> {
  return { past: [], present: t, future: [] };
}

/** Records `t` as the new present. Clears redo; drops the oldest undo step past the cap. */
export function push<T>(h: History<T>, t: T): History<T> {
  if (t === h.present) return h;
  const past = [...h.past, h.present];
  if (past.length > HISTORY_CAP) past.splice(0, past.length - HISTORY_CAP);
  return { past, present: t, future: [] };
}

export function undo<T>(h: History<T>): History<T> {
  if (h.past.length === 0) return h;
  return {
    past: h.past.slice(0, -1),
    present: h.past[h.past.length - 1],
    future: [h.present, ...h.future],
  };
}

export function redo<T>(h: History<T>): History<T> {
  if (h.future.length === 0) return h;
  return {
    past: [...h.past, h.present],
    present: h.future[0],
    future: h.future.slice(1),
  };
}

export function isEdited<T>(h: History<T>): boolean {
  return h.past.length > 0;
}
