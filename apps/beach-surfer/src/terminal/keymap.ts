function trace(...args: unknown[]): void {
  if (typeof window !== 'undefined' && (window as any).__BEACH_TRACE) {
    console.debug('[beach-trace][keymap]', ...args);
  }
}

export function encodeKeyEvent(event: KeyboardEvent): Uint8Array | null {
  if (event.metaKey) {
    trace('encodeKeyEvent: metaKey pressed, ignoring', { key: event.key });
    return null;
  }

  switch (event.key) {
    case 'Enter': {
      const hasShiftOnly = event.shiftKey && !event.ctrlKey && !event.altKey;
      if (hasShiftOnly) {
        trace('encodeKeyEvent: Shift+Enter', { result: '0x0a' });
        return new Uint8Array([0x0a]);
      }
      trace('encodeKeyEvent: Enter key', { result: '0x0d' });
      return new Uint8Array([0x0d]);
    }
    case 'Tab':
      trace('encodeKeyEvent: Tab key', { result: '0x09' });
      return new Uint8Array([0x09]);
    case 'Backspace':
      trace('encodeKeyEvent: Backspace key', { result: '0x7f' });
      return new Uint8Array([0x7f]);
    case 'Escape':
      trace('encodeKeyEvent: Escape key', { result: '0x1b' });
      return new Uint8Array([0x1b]);
    case 'ArrowUp':
      return esc('[A');
    case 'ArrowDown':
      return esc('[B');
    case 'ArrowRight':
      return esc('[C');
    case 'ArrowLeft':
      return esc('[D');
    case 'Home':
      return esc('[H');
    case 'End':
      return esc('[F');
    case 'PageUp':
      return esc('[5~');
    case 'PageDown':
      return esc('[6~');
    case 'Delete':
      return esc('[3~');
    case 'Insert':
      return esc('[2~');
    default:
      break;
  }

  if (event.key.length === 1) {
    const char = event.key;
    const bytes: number[] = [];
    const lower = char.toLowerCase();
    if (event.ctrlKey) {
      if (lower >= 'a' && lower <= 'z') {
        bytes.push(lower.charCodeAt(0) - 96);
        trace('encodeKeyEvent: Ctrl+letter', { key: event.key, char, lower, bytes });
      } else if (char === ' ' || char === '@') {
        bytes.push(0);
        trace('encodeKeyEvent: Ctrl+space/@', { key: event.key, char, bytes });
      } else {
        trace('encodeKeyEvent: unhandled Ctrl+key', { key: event.key, char });
        return null;
      }
    } else {
      const encoder = new TextEncoder();
      bytes.push(...encoder.encode(char));
      trace('encodeKeyEvent: regular character', { key: event.key, char, bytes });
    }
    if (event.altKey) {
      bytes.unshift(0x1b);
      trace('encodeKeyEvent: Alt modifier applied', { bytes });
    }
    const result = new Uint8Array(bytes);
    const debugStr = Array.from(result).map(b => `0x${b.toString(16).padStart(2, '0')}`).join(' ');
    trace('encodeKeyEvent: final result', { key: event.key, result: debugStr });
    return result;
  }

  trace('encodeKeyEvent: no encoding for key', { key: event.key, keyLength: event.key.length });
  return null;
}

function esc(sequence: string): Uint8Array {
  const encoder = new TextEncoder();
  return encoder.encode(`\u001b${sequence}`);
}
