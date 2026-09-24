import { test, expect } from "./fixtures";

test("navigation links open the matching section", async ({ page }) => {
  const sections = [
    { link: "Recordings", path: "/recordings", heading: "Recordings" },
    { link: "Search", path: "/search", heading: "Search Transcripts" },
    { link: "Settings", path: "/settings", heading: "Settings" },
    { link: "Recorder", path: "/", heading: "Meeting Recorder" },
  ] as const;

  await page.goto("/");
  await expect(page).toHaveURL((url) => url.pathname === "/");

  for (const section of sections) {
    await page.getByRole("link", { name: section.link }).click();
    await expect(page).toHaveURL((url) => url.pathname === section.path);
    await expect(page.getByRole("heading", { name: section.heading })).toBeVisible();
  }
});
