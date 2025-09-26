export function encodeKeyEvent(event: KeyboardEvent): Uint8Array | null {
  if (event.metaKey) {
    return null;
  }

  switch (event.key) {
    case 'Enter':
      return new Uint8Array([0x0a]);
    case 'Tab':
      return new Uint8Array([0x09]);
    case 'Backspace':
      return new Uint8Array([0x7f]);
    case 'Escape':
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
      } else if (char === ' ' || char === '@') {
        bytes.push(0);
      } else {
        return null;
      }
    } else {
      const encoder = new TextEncoder();
      bytes.push(...encoder.encode(char));
    }
    if (event.altKey) {
      bytes.unshift(0x1b);
    }
    return new Uint8Array(bytes);
  }

  return null;
}

function esc(sequence: string): Uint8Array {
  const encoder = new TextEncoder();
  return encoder.encode(`\u001b${sequence}`);
}
