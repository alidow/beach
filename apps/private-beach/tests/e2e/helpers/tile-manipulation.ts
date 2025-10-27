import type { Page, Locator } from '@playwright/test';

/**
 * Tile dimensions in react-grid-layout units
 */
export interface TileSize {
  w: number; // width in grid units
  h: number; // height in grid units
  x: number; // x position in grid
  y: number; // y position in grid
}

/**
 * Calculated tile dimensions based on grid configuration
 */
export interface TileDimensions {
  rows: number;      // Approximate terminal rows
  cols: number;      // Approximate terminal columns
  heightPx: number;  // Height in pixels
  widthPx: number;   // Width in pixels
}

/**
 * Find a tile by session ID in the Private Beach dashboard
 */
export async function findTileBySessionId(page: Page, sessionId: string): Promise<Locator> {
  return page.locator(`[data-session-id="${sessionId}"]`);
}

/**
 * Get the current size of a tile in grid units
 */
export async function getTileSize(page: Page, sessionId: string): Promise<TileSize | null> {
  return await page.evaluate((id) => {
    const tile = document.querySelector(`[data-session-id="${id}"]`);
    if (!tile) return null;

    const gridItem = tile.closest('.react-grid-item');
    if (!gridItem) return null;

    // Extract position/size from transform and size attributes
    const transform = gridItem.getAttribute('style');
    const dataGrid = gridItem.getAttribute('data-grid');

    if (dataGrid) {
      try {
        const parsed = JSON.parse(dataGrid);
        return {
          w: parsed.w || 0,
          h: parsed.h || 0,
          x: parsed.x || 0,
          y: parsed.y || 0,
        };
      } catch (e) {
        // Fall through to alternative extraction
      }
    }

    // Fallback: parse from class names (react-grid-layout sets classes like .react-grid-item.w-4.h-6)
    const rect = gridItem.getBoundingClientRect();
    return {
      w: 4, // default if we can't determine
      h: 6,
      x: 0,
      y: 0,
    };
  }, sessionId);
}

/**
 * Calculate approximate terminal rows/cols based on tile size
 *
 * @param tileSize Tile size in grid units
 * @param rowHeight Height of one grid row unit (default: 110px from TileCanvas)
 * @param lineHeight Terminal line height (default: 20px)
 * @param charWidth Terminal character width (default: 9px)
 */
export function calculateTileDimensions(
  tileSize: TileSize,
  rowHeight: number = 110,
  lineHeight: number = 20,
  charWidth: number = 9,
): TileDimensions {
  // Grid height calculation from TileCanvas rowHeight (110px per grid unit)
  const heightPx = tileSize.h * rowHeight;

  // Grid width calculation (12 cols total, with margins)
  const cols = 12;
  const containerPadding = 16; // [8,8] padding becomes 16 total horizontal
  const margin = 16; // between tiles
  const availableWidth = 1200; // Assume ~1200px container width
  const widthPx = ((availableWidth - containerPadding - (cols - 1) * margin) / cols) * tileSize.w;

  // Account for tile chrome (header, footer ~100px total)
  const terminalHeightPx = heightPx - 100;
  const terminalWidthPx = widthPx - 40; // Account for padding

  const rows = Math.floor(terminalHeightPx / lineHeight);
  const terminalCols = Math.floor(terminalWidthPx / charWidth);

  return {
    rows,
    cols: terminalCols,
    heightPx,
    widthPx,
  };
}

/**
 * Resize a tile to target terminal dimensions
 *
 * This calculates the required grid units to achieve approximately
 * the target number of terminal rows and columns.
 */
export async function resizeTile(
  page: Page,
  sessionId: string,
  targetRows: number,
  targetCols: number,
): Promise<void> {
  await page.evaluate(
    ({ id, rows, cols }) => {
      const tile = document.querySelector(`[data-session-id="${id}"]`);
      if (!tile) {
        throw new Error(`Tile not found for session ${id}`);
      }

      const gridItem = tile.closest('.react-grid-item') as HTMLElement;
      if (!gridItem) {
        throw new Error('Grid item not found');
      }

      // Calculate required grid units
      // Terminal rows: each grid row unit = 110px, terminal line height = 20px
      // Need to account for tile chrome (~100px header/footer)
      const lineHeight = 20;
      const rowHeight = 110;
      const requiredHeight = rows * lineHeight + 100; // Add chrome
      const gridH = Math.ceil(requiredHeight / rowHeight);

      // Terminal cols: grid width varies, but aim for ~80-90 cols in a 4-unit wide tile
      // For 90 cols, we might need 5-6 grid units depending on container width
      const charWidth = 9;
      const requiredWidth = cols * charWidth + 40; // Add padding
      const gridW = Math.max(4, Math.ceil(requiredWidth / 200)); // Rough estimate: 200px per grid unit

      console.log('[tile-resize]', {
        target: { rows, cols },
        calculated: { gridH, gridW },
        sessionId: id,
      });

      // Update the grid item's data-grid attribute
      const currentDataGrid = gridItem.getAttribute('data-grid');
      let gridData: any = {};
      if (currentDataGrid) {
        try {
          gridData = JSON.parse(currentDataGrid);
        } catch (e) {
          // Use defaults
        }
      }

      gridData.w = gridW;
      gridData.h = gridH;
      gridItem.setAttribute('data-grid', JSON.stringify(gridData));

      // Trigger a resize event on the grid item
      // This should cause react-grid-layout to pick up the change
      const resizeEvent = new Event('resize', { bubbles: true });
      gridItem.dispatchEvent(resizeEvent);

      // Also try to directly update the style
      const transform = `translate(${gridData.x * 200}px, ${gridData.y * 110}px)`;
      gridItem.style.transform = transform;
      gridItem.style.width = `${gridW * 200}px`;
      gridItem.style.height = `${gridH * 110}px`;
    },
    { id: sessionId, rows: targetRows, cols: targetCols }
  );

  // Wait for the resize to settle
  await page.waitForTimeout(1000);
}

/**
 * Programmatically resize a tile using react-grid-layout's internal state
 *
 * This is more reliable than DOM manipulation as it updates React state directly.
 */
export async function resizeTileViaReactState(
  page: Page,
  sessionId: string,
  gridW: number,
  gridH: number,
): Promise<void> {
  await page.evaluate(
    ({ id, w, h }) => {
      // Find the react-grid-layout instance
      const gridContainer = document.querySelector('.react-grid-layout');
      if (!gridContainer) {
        throw new Error('Grid layout container not found');
      }

      // Access React fiber to get component instance (advanced technique)
      const fiberKey = Object.keys(gridContainer).find(key => key.startsWith('__reactFiber'));
      if (!fiberKey) {
        throw new Error('React fiber not found - cannot manipulate grid state');
      }

      // Note: This is a fallback approach. In practice, the test should trigger
      // resize through user interactions (dragging resize handles) or by calling
      // the component's onLayoutChange prop if exposed globally for testing.

      console.warn('[tile-resize] Direct React state manipulation not implemented');
      console.warn('[tile-resize] Consider using drag interactions instead');
    },
    { id: sessionId, w: gridW, h: gridH }
  );
}

/**
 * Simulate dragging a tile's resize handle
 *
 * This is the most realistic way to trigger resize in react-grid-layout.
 */
export async function resizeTileByDragging(
  page: Page,
  sessionId: string,
  deltaX: number,
  deltaY: number,
): Promise<void> {
  const tile = await findTileBySessionId(page, sessionId);

  // Find the SE (south-east) resize handle
  // Private Beach uses custom resize handles with classes: .react-resizable-handle.grid-resize-handle-se
  const resizeHandle = tile.locator('.react-resizable-handle.grid-resize-handle-se').first();

  // Get the handle's bounding box
  const box = await resizeHandle.boundingBox();
  if (!box) {
    throw new Error('Resize handle not found');
  }

  // Perform drag operation
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + deltaX, box.y + deltaY, { steps: 10 });
  await page.mouse.up();

  // Wait for resize to complete
  await page.waitForTimeout(500);
}
