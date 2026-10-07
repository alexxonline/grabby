import { test, expect } from "./fixtures";

test("saves recording preferences", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Settings" }).click();

  const autoDelete = page.getByRole("switch", { name: "Auto-delete audio" });
  const autoTitle = page.getByRole("switch", { name: "Auto title" });
  await expect(autoDelete).not.toBeChecked();
  await expect(autoTitle).not.toBeChecked();
  await expect(page.getByText("Add OPENROUTER_API_KEY")).toBeVisible();

  await autoDelete.click();
  await expect(autoDelete).toBeChecked();
  await expect(page.getByRole("status").filter({ hasText: "Settings saved." })).toHaveText("Settings saved.");

  await autoTitle.click();
  await expect(autoTitle).toBeChecked();
  await expect(page.getByRole("status").filter({ hasText: "Settings saved." })).toHaveText("Settings saved.");

  await page.reload();
  await expect(autoDelete).toBeChecked();
  await expect(autoTitle).toBeChecked();
});
