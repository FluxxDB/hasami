import { describe, expect, it } from 'vitest';
import { keyToCommand, type Command } from './keys';

const k = (key: string, mods: { shift?: boolean; ctrl?: boolean; meta?: boolean } = {}) =>
  keyToCommand({ key, shiftKey: !!mods.shift, ctrlKey: !!mods.ctrl, metaKey: !!mods.meta });

describe('keyToCommand (spec 5.5)', () => {
  it('Space: play / pause', () => {
    expect(k(' ')).toBe<Command>('playPause');
  });
  it('J / K / L, either case', () => {
    expect(k('j')).toBe<Command>('J');
    expect(k('k')).toBe<Command>('K');
    expect(k('l')).toBe<Command>('L');
    expect(k('J', { shift: true })).toBe<Command>('J');
    expect(k('K')).toBe<Command>('K');
    expect(k('L')).toBe<Command>('L');
  });
  it('Left / Right: one frame', () => {
    expect(k('ArrowLeft')).toBe<Command>('frameBack');
    expect(k('ArrowRight')).toBe<Command>('frameFwd');
  });
  it('Shift + Left / Right: one second', () => {
    expect(k('ArrowLeft', { shift: true })).toBe<Command>('secBack');
    expect(k('ArrowRight', { shift: true })).toBe<Command>('secFwd');
  });
  it('Up / Down: previous / next cut', () => {
    expect(k('ArrowUp')).toBe<Command>('prevCut');
    expect(k('ArrowDown')).toBe<Command>('nextCut');
  });
  it('Home / End', () => {
    expect(k('Home')).toBe<Command>('home');
    expect(k('End')).toBe<Command>('end');
  });
  it('Ctrl+B: cut', () => {
    expect(k('b', { ctrl: true })).toBe<Command>('cut');
    expect(k('B', { ctrl: true })).toBe<Command>('cut');
  });
  it('Delete / Backspace: delete', () => {
    expect(k('Delete')).toBe<Command>('delete');
    expect(k('Backspace')).toBe<Command>('delete');
  });
  it('Ctrl+Z undo, Ctrl+Shift+Z redo', () => {
    expect(k('z', { ctrl: true })).toBe<Command>('undo');
    expect(k('Z', { ctrl: true, shift: true })).toBe<Command>('redo');
    expect(k('z', { ctrl: true, shift: true })).toBe<Command>('redo');
  });
  it('N: toggle snapping', () => {
    expect(k('n')).toBe<Command>('toggleSnap');
    expect(k('N')).toBe<Command>('toggleSnap');
  });
  it('Ctrl+Shift+L: toggle linked selection', () => {
    expect(k('L', { ctrl: true, shift: true })).toBe<Command>('toggleLinked');
    expect(k('l', { ctrl: true, shift: true })).toBe<Command>('toggleLinked');
  });

  it('Cmd (meta) works like Ctrl', () => {
    expect(k('z', { meta: true })).toBe<Command>('undo');
    expect(k('b', { meta: true })).toBe<Command>('cut');
  });
  it('keys that are not bound return null', () => {
    expect(k('b')).toBeNull();
    expect(k('z')).toBeNull();
    expect(k('x')).toBeNull();
    expect(k('Enter')).toBeNull();
    expect(k('l', { ctrl: true })).toBeNull(); // Ctrl+L alone is not Ctrl+Shift+L
    expect(k('n', { ctrl: true })).toBeNull();
    expect(k('ArrowLeft', { ctrl: true })).toBeNull();
    expect(keyToCommand({ key: 'b', ctrlKey: false, shiftKey: false })).toBeNull();
  });
});
