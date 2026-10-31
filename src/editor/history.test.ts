import { describe, expect, it } from 'vitest';
import { HISTORY_CAP, initHistory, isEdited, push, redo, undo } from './history';

describe('history', () => {
  it('history_starts_unedited', () => {
    const h = initHistory(0);
    expect(h).toEqual({ past: [], present: 0, future: [] });
    expect(isEdited(h)).toBe(false);
  });

  it('history_undo_redo_restore_states', () => {
    let h = initHistory(0);
    h = push(h, 1);
    h = push(h, 2);
    expect(isEdited(h)).toBe(true);
    h = undo(h);
    expect(h.present).toBe(1);
    h = undo(h);
    expect(h.present).toBe(0);
    expect(isEdited(h)).toBe(false);
    h = redo(h);
    expect(h.present).toBe(1);
    h = redo(h);
    expect(h.present).toBe(2);
  });

  it('history_undo_and_redo_at_the_ends_are_no_ops', () => {
    const h = initHistory(0);
    expect(undo(h)).toBe(h);
    expect(redo(h)).toBe(h);
  });

  it('history_push_after_undo_clears_future', () => {
    let h = initHistory(0);
    h = push(push(h, 1), 2);
    h = undo(h);
    expect(h.future).toEqual([2]);
    h = push(h, 3);
    expect(h.future).toEqual([]);
    expect(h.present).toBe(3);
    expect(redo(h)).toBe(h);
  });

  it('history_push_of_the_same_state_is_a_no_op', () => {
    const state = { a: 1 };
    const h = initHistory(state);
    expect(push(h, state)).toBe(h);
  });

  it('history_201st_push_drops_the_oldest', () => {
    let h = initHistory(0);
    for (let i = 1; i <= HISTORY_CAP + 1; i++) h = push(h, i);
    expect(h.past.length).toBe(HISTORY_CAP);
    expect(h.past[0]).toBe(1); // state 0 fell off the front
    expect(h.present).toBe(HISTORY_CAP + 1);
    for (let i = 0; i < HISTORY_CAP; i++) h = undo(h);
    expect(h.present).toBe(1);
    expect(isEdited(h)).toBe(false);
  });
});
