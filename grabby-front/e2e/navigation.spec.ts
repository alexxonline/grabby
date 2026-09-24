import { test, expect } from "./fixtures";

test("reaches each section from the navigation bar", async ({ page }, testInfo) => {
  const shot = async (name: string) => {
    await testInfo.attach(name, {
      body: await page.screenshot(),
      contentType: "image/png",
    });
  };

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Meeting Recorder" })).toBeVisible();
  await shot("recorder");

  await page.getByRole("link", { name: "Recordings" }).click();
  await expect(page.getByRole("heading", { name: "Recordings" })).toBeVisible();
  await expect(page.getByText("Sprint planning")).toBeVisible();
  await shot("recordings");

  await page.getByRole("link", { name: "Search" }).click();
  await expect(page.getByRole("heading", { name: "Search Transcripts" })).toBeVisible();
  await shot("search");

  await page.getByRole("link", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("switch", { name: "Auto-delete audio" })).toBeVisible();
  await shot("settings");

  await page.getByRole("link", { name: "Recorder" }).click();
  await expect(page.getByRole("heading", { name: "Meeting Recorder" })).toBeVisible();
});
