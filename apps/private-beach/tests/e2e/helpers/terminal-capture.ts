import type { Page } from '@playwright/test';

/**
 * Terminal row representation
 */
export interface TerminalRow {
  kind: 'loaded' | 'missing';
  absolute: number;
  text: string;
  seq?: number;
}

/**
 * Terminal state snapshot
 */
export interface TerminalState {
  viewportRows: number;
  gridRows: number;
  gridCols: number;
  visibleRowCount: number;
  rows: TerminalRow[];
  baseRow: number;
  followTail: boolean;
}

/**
 * Duplicate pattern found in terminal
 */
export interface DuplicatePattern {
  text: string;
  firstIndex: number;
  secondIndex: number;
}

/**
 * Analysis result for duplicate detection
 */
export interface DuplicateAnalysis {
  duplicates: DuplicatePattern[];
  suspiciousPatterns: string[];
}

/**
 * Capture the current terminal state including all visible rows
 * Uses the Beach trace API (__BEACH_TRACE_DUMP_ROWS) to get detailed row data
 */
export async function captureTerminalState(page: Page, label: string): Promise<TerminalState> {
  const state = await page.evaluate(() => {
    // Use the Beach trace API to dump rows
    if (typeof (window as any).__BEACH_TRACE_DUMP_ROWS === 'function') {
      (window as any).__BEACH_TRACE_DUMP_ROWS();
      const traceData = (window as any).__BEACH_TRACE_LAST_ROWS;

      if (traceData) {
        return {
          viewportRows: traceData.viewportHeight,
          gridRows: traceData.rowCount,
          gridCols: 80, // Beach uses 80 cols by default
          visibleRowCount: traceData.rows.length,
          baseRow: traceData.baseRow,
          followTail: traceData.followTail,
          rows: traceData.rows.map((row: any) => ({
            kind: row.kind,
            absolute: row.absolute,
            text: row.text || '',
            seq: row.seq,
          })),
        };
      }
    }
    return null;
  });

  if (!state) {
    // Fallback: extract text from DOM if trace API is not available
    const rows = await page.locator('[data-session-id] .xterm-row').all();
    const lines = await Promise.all(rows.map(row => row.textContent()));

    return {
      viewportRows: lines.length,
      gridRows: lines.length,
      gridCols: 80, // default guess
      visibleRowCount: lines.length,
      baseRow: 0,
      followTail: true,
      rows: lines.map((text, index) => ({
        kind: 'loaded' as const,
        absolute: index,
        text: text || '',
      })),
    };
  }

  return state;
}

/**
 * Analyze terminal grid for duplicate HUD patterns
 *
 * Detects duplicate content that might indicate the PTY resize bug.
 * Ignores natural repetition (like Pong game borders).
 */
export function analyzeGridForDuplicates(rows: TerminalRow[]): DuplicateAnalysis {
  const duplicates: DuplicatePattern[] = [];
  const suspiciousPatterns: string[] = [];

  // Common HUD patterns to watch for (from Pong demo)
  const hudPatterns = [
    'Unknown command',
    'Commands:',
    'Mode:',
    'Ready.',
  ];

  // Patterns to ignore (Pong game borders that naturally repeat)
  const ignorePatterns = [
    /^\|[\s|]*\|$/,  // Border rows like "|      |"
  ];

  // Check for consecutive identical non-blank rows
  for (let i = 0; i < rows.length - 1; i++) {
    const current = rows[i];
    const next = rows[i + 1];

    if (current.kind !== 'loaded' || next.kind !== 'loaded') {
      continue;
    }

    const currentText = current.text.trim();
    const nextText = next.text.trim();

    // Skip blank rows
    if (currentText === '' || nextText === '') {
      continue;
    }

    // Skip if both rows match an ignore pattern (e.g., Pong borders)
    const shouldIgnore = ignorePatterns.some(pattern =>
      pattern.test(currentText) && pattern.test(nextText)
    );
    if (shouldIgnore) {
      continue;
    }

    // Check for exact duplicates
    if (currentText === nextText) {
      duplicates.push({
        text: currentText,
        firstIndex: i,
        secondIndex: i + 1,
      });
    }

    // Check for HUD patterns appearing multiple times
    for (const pattern of hudPatterns) {
      if (currentText.includes(pattern)) {
        const count = rows.filter(row =>
          row.kind === 'loaded' && row.text.includes(pattern)
        ).length;

        if (count > 1 && !suspiciousPatterns.includes(pattern)) {
          suspiciousPatterns.push(pattern);
        }
      }
    }
  }

  return { duplicates, suspiciousPatterns };
}

/**
 * Enable Beach trace logging on the page
 * Must be called before navigating to the page
 */
export async function enableTraceLogging(page: Page): Promise<void> {
  await page.addInitScript(() => {
    (window as any).__BEACH_TRACE = true;
  });
}
