import { twMerge } from 'tailwind-merge';

type ClassDictionary = Record<string, boolean | null | undefined>;
type ClassArray = ClassValue[];
export type ClassValue = ClassArray | ClassDictionary | string | number | boolean | null | undefined;

function toClassString(mix: ClassValue): string {
  if (!mix && mix !== 0) {
    return '';
  }
  if (typeof mix === 'string' || typeof mix === 'number') {
    return String(mix);
  }
  if (Array.isArray(mix)) {
    let result = '';
    for (const value of mix) {
      const next = toClassString(value);
      if (!next) {
        continue;
      }
      if (result.length > 0) {
        result += ' ';
      }
      result += next;
    }
    return result;
  }
  if (typeof mix === 'object') {
    let result = '';
    for (const key of Object.keys(mix)) {
      if (!key || !mix[key]) {
        continue;
      }
      if (result.length > 0) {
        result += ' ';
      }
      result += key;
    }
    return result;
  }
  return '';
}

function clsxLite(...inputs: ClassValue[]): string {
  let result = '';
  for (const input of inputs) {
    const next = toClassString(input);
    if (!next) {
      continue;
    }
    if (result.length > 0) {
      result += ' ';
    }
    result += next;
  }
  return result;
}

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsxLite(...inputs));
}
