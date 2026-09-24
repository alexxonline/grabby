import { test, expect } from "./fixtures";

test("shows a ready recorder", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Meeting Recorder" })).toBeVisible();
  await expect(page.getByText("Ready", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Start Recording" })).toBeEnabled();
  await expect(page.getByPlaceholder("Recording name (optional)")).toBeVisible();
});

test("starts and stops a meeting without capturing audio", async ({ page }) => {
  await page.goto("/");
  await page.getByPlaceholder("Recording name (optional)").fill("Quarterly review");
  await page.getByRole("button", { name: "Start Recording" }).click();

  await expect(page.getByText("Recording", { exact: true })).toBeVisible();
  await expect(page.locator(".timer")).toHaveText(/\d{2}:\d{2}:\d{2}/);
  await expect(page.getByText("File: quarterly-review.mp3")).toBeVisible();
  await expect(page.getByRole("log", { name: "Live transcript" })).toContainText("Hello from the test.");
  await expect(page.getByText("joining now")).toBeVisible();

  await page.getByRole("button", { name: "Stop Recording" }).click();
  await expect(page.getByText("Recording saved")).toBeVisible();
  await expect(page.getByText("Recording finished")).toBeVisible();

  await page.getByRole("link", { name: "View recordings" }).click();
  await expect(page.getByRole("heading", { name: "Recordings" })).toBeVisible();
  await expect(page.getByText("Quarterly review")).toBeVisible();
});

test.describe("recorder unavailable", () => {
  test.use({ mockApi: { statusFails: true } });

  test("shows a reconnecting message when status cannot be loaded", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Cannot reach the recorder. Reconnecting…")).toBeVisible();
  });
});
