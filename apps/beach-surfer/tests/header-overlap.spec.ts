import { test, expect } from '@playwright/test';

test.describe('Terminal Header Overlap', () => {
  test('should not overlap header with terminal content before connection', async ({ page }) => {
    // Navigate to the beach-surfer app
    await page.goto('http://localhost:5173/');

    // Wait for the page to load
    await page.waitForLoadState('networkidle');

    // Find the header element
    const header = page.locator('header');
    await expect(header).toBeVisible();

    // Find the terminal container
    const terminal = page.locator('.beach-terminal');
    await expect(terminal).toBeVisible();

    // Find the idle placeholder
    const idlePlaceholder = page.locator('text=Terminal idle');
    await expect(idlePlaceholder).toBeVisible();

    // Get bounding boxes
    const headerBox = await header.boundingBox();
    const terminalBox = await terminal.boundingBox();
    const placeholderBox = await idlePlaceholder.boundingBox();

    // Get the placeholder parent's bounding box
    const placeholderParent = idlePlaceholder.locator('xpath=ancestor::div[@class and contains(@class, "pointer-events-none")]');
    const parentBox = await placeholderParent.boundingBox();

    // Log the positions
    console.log('Header box:', headerBox);
    console.log('Terminal box:', terminalBox);
    console.log('Placeholder parent box:', parentBox);
    console.log('Placeholder box:', placeholderBox);

    // Check if header and placeholder overlap
    if (headerBox && placeholderBox) {
      const headerBottom = headerBox.y + headerBox.height;
      const placeholderTop = placeholderBox.y;

      console.log('Header bottom:', headerBottom);
      console.log('Placeholder top:', placeholderTop);
      console.log('Gap between header and placeholder:', placeholderTop - headerBottom);

      // The placeholder should be below the header (no overlap)
      expect(placeholderTop).toBeGreaterThanOrEqual(headerBottom);

      // If they overlap, this test will fail
      if (placeholderTop < headerBottom) {
        console.error('OVERLAP DETECTED: Header overlaps with placeholder');
        console.error(`Overlap amount: ${headerBottom - placeholderTop}px`);
      }
    }

    // Take a screenshot for visual inspection
    await page.screenshot({ path: 'test-results/header-overlap-fixed.png', fullPage: true });
  });

  test('should show correct z-index stacking', async ({ page }) => {
    await page.goto('http://localhost:5173/');
    await page.waitForLoadState('networkidle');

    // Check computed styles
    const header = page.locator('header');
    const terminal = page.locator('.beach-terminal');

    const headerZIndex = await header.evaluate((el) => {
      return window.getComputedStyle(el).zIndex;
    });

    const terminalZIndex = await terminal.evaluate((el) => {
      return window.getComputedStyle(el).zIndex;
    });

    console.log('Header z-index:', headerZIndex);
    console.log('Terminal z-index:', terminalZIndex);
  });
});
