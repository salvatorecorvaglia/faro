import { expect, test } from '@playwright/test';

test('real modal bounds, focus trapping, Escape and focus restoration', async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await page.goto('/tests/browser/harness.html?case=modal');
  const trigger = page.getByRole('button', { name: 'Open dialog' });
  await trigger.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('modal.png') });
  const bounds = await dialog.boundingBox();
  expect(bounds!.y).toBeGreaterThanOrEqual(0);
  expect(bounds!.height).toBeLessThanOrEqual(540);
  await page.getByRole('button', { name: 'Done', exact: true }).focus();
  for (let index = 0; index < 6; index++) {
    await page.keyboard.press('Tab');
    await expect(trigger).not.toBeFocused();
    expect(
      await page.evaluate(
        () =>
          document.activeElement === document.body ||
          !!document.activeElement?.closest('dialog[open]'),
      ),
    ).toBe(true);
  }
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(trigger).toBeFocused();
});

test('wide grid virtualizes columns and keeps keyboard selection visible', async ({ page }) => {
  await page.goto('/tests/browser/harness.html?columns=1000&rows=100');
  const grid = page.getByRole('grid');
  await expect(grid).toBeVisible();
  await expect(
    page.getByRole('columnheader', { name: 'column_0 text', exact: true }),
  ).toBeVisible();
  expect(await page.getByRole('columnheader').count()).toBeLessThan(30);
  expect(await page.getByRole('gridcell').count()).toBeLessThan(2000);
  await grid.focus();
  await page.keyboard.press('End');
  const selected = page.locator('[role="gridcell"][aria-selected="true"]');
  await expect(selected).toHaveCount(1);
  await expect(selected).toHaveAttribute('aria-colindex', '1000');
  await expect(selected).toBeInViewport();
  await page.keyboard.press('Home');
  await expect(selected).toHaveAttribute('aria-colindex', '1');
  await expect(selected).toBeInViewport();
  await page.getByRole('button', { name: 'Column filters' }).click();
  const filter = page.getByRole('textbox', { name: 'Filter column_0', exact: true });
  await filter.fill('abc');
  await filter.press('ArrowLeft');
  expect(await filter.evaluate((element: HTMLInputElement) => element.selectionStart)).toBe(2);
});

test('large grid worker sorts correctly while the UI keeps processing frames', async ({ page }) => {
  await page.goto('/tests/browser/harness.html?rows=50000&columns=4');
  await expect(page.getByRole('grid')).toBeVisible();
  await page.evaluate(() => {
    const state = { ticks: 0, start: performance.now() };
    (window as unknown as { measure: typeof state }).measure = state;
    const frame = () => {
      state.ticks++;
      if (performance.now() - state.start < 2000) requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  });
  await page.getByRole('columnheader').first().click();
  await expect(page.getByText('Updating rows…')).toHaveCount(0);
  await expect(page.getByRole('row').first().getByRole('gridcell').first()).toHaveText('Item 0');
  await page.getByRole('columnheader').first().click();
  await expect(page.getByRole('row').first().getByRole('gridcell').first()).toHaveText(
    'Item 49999',
  );
  const measurement = await page.evaluate(() => ({
    ...(window as unknown as { measure: { ticks: number; start: number } }).measure,
    finish: performance.now(),
  }));
  expect(measurement.ticks).toBeGreaterThan(1);
  console.log('large-grid browser measurement', {
    frames: measurement.ticks,
    elapsedMs: measurement.finish - measurement.start,
    cells: await page.getByRole('gridcell').count(),
  });
});
