// DaVinci-style keyboard map for the Quick Trim editor (spec 5.5). Turns a keydown into a
// command name; the editor decides what each command does.

export type Command =
  | 'playPause'
  | 'J'
  | 'K'
  | 'L'
  | 'frameBack'
  | 'frameFwd'
  | 'secBack'
  | 'secFwd'
  | 'prevCut'
  | 'nextCut'
  | 'home'
  | 'end'
  | 'cut'
  | 'delete'
  | 'undo'
  | 'redo'
  | 'toggleSnap'
  | 'toggleLinked';

export function keyToCommand(e: {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey?: boolean;
}): Command | null {
  const letter = e.key.length === 1 ? e.key.toLowerCase() : '';

  if (e.ctrlKey || e.metaKey) {
    if (letter === 'z') return e.shiftKey ? 'redo' : 'undo';
    if (letter === 'b' && !e.shiftKey) return 'cut';
    if (letter === 'l' && e.shiftKey) return 'toggleLinked';
    return null;
  }

  switch (e.key) {
    case ' ':
      return 'playPause';
    case 'ArrowLeft':
      return e.shiftKey ? 'secBack' : 'frameBack';
    case 'ArrowRight':
      return e.shiftKey ? 'secFwd' : 'frameFwd';
    case 'ArrowUp':
      return 'prevCut';
    case 'ArrowDown':
      return 'nextCut';
    case 'Home':
      return 'home';
    case 'End':
      return 'end';
    case 'Delete':
    case 'Backspace':
      return 'delete';
  }

  // The letter itself carries the case, so Shift+J (or Caps Lock) is still J.
  if (letter === 'j') return 'J';
  if (letter === 'k') return 'K';
  if (letter === 'l') return 'L';
  if (letter === 'n') return 'toggleSnap';
  return null;
}
