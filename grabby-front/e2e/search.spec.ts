import { test, expect } from "./fixtures";

test("finds a transcript and highlights the match", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Search" }).click();
  const input = page.getByPlaceholder("Search transcripts…");
  await expect(page.getByRole("button", { name: "Search" })).toBeDisabled();
  await input.fill("editor");
  await page.getByRole("button", { name: "Search" }).click();

  const result = page.getByRole("link", { name: /Sprint planning/ });
  await expect(result).toBeVisible();
  await expect(result.locator("mark")).toHaveText("editor");
});

test("shows an empty result for an unknown phrase", async ({ page }) => {
  await page.goto("/search");
  await page.getByPlaceholder("Search transcripts…").fill("zebra");
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page.getByText('No results found for "zebra"')).toBeVisible();
});
