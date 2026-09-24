import { test, expect } from "./fixtures";

test("renames a speaker in a saved transcript", async ({ page }) => {
  await page.goto("/recordings");
  await page.locator(".recording-item", { hasText: "Sprint planning" })
    .getByRole("button", { name: /Transcript/ })
    .click();

  await expect(page.getByRole("heading", { name: "Sprint planning" })).toBeVisible();
  await expect(page.getByText("We should ship the notes editor.")).toBeVisible();

  await page.getByRole("button", { name: "Alice", exact: true }).click();
  const input = page.locator(".speaker-chip-input");
  await input.fill("Alex");
  await input.press("Enter");

  await expect(page.getByRole("button", { name: "Alex", exact: true })).toBeVisible();
  await expect(page.getByText("Alex: We should ship the notes editor.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Alice", exact: true })).toHaveCount(0);
});

test("rejects a speaker name that contains a colon", async ({ page }) => {
  await page.goto("/transcription/planning.mp3");
  await page.getByRole("button", { name: "Bob", exact: true }).click();
  const input = page.locator(".speaker-chip-input");
  await input.fill("Bad:Name");
  await input.press("Enter");
  await expect(page.getByText("Speaker names cannot contain colons or line breaks.")).toBeVisible();
  await expect(input).toHaveValue("Bad:Name");
  await expect(page.getByText("Bob: Sounds good.")).toBeVisible();
});

test("saves notes for a recording", async ({ page }) => {
  await page.goto("/recordings");
  await page.locator(".recording-item", { hasText: "Sprint planning" })
    .getByRole("link", { name: "Notes" })
    .click();

  const notes = page.getByLabel("Your notes");
  await expect(notes).toHaveValue("Bring the roadmap.");
  await notes.fill("Bring the roadmap.\nSend the notes.");
  await page.getByRole("button", { name: "Save notes" }).click();
  await expect(page.getByRole("status")).toHaveText("Saved");
  await expect(notes).toHaveValue("Bring the roadmap.\nSend the notes.");
});
