import { test, expect } from "./fixtures";

test("lists saved meetings, including transcript-only entries", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Recordings" }).click();

  await expect(page.getByText("Sprint planning")).toBeVisible();
  await expect(page.getByText("Customer interview")).toBeVisible();
  const standup = page.locator(".recording-item", { hasText: "Standup" });
  await expect(standup.getByText("Transcript only")).toBeVisible();
  await expect(standup.getByTitle("Play")).toHaveCount(0);
  await expect(standup.getByRole("link", { name: "Download" })).toHaveCount(0);
});

test.describe("empty library", () => {
  test.use({ mockApi: { library: "empty" } });

  test("shows an empty library", async ({ page }) => {
    await page.goto("/recordings");
    await expect(page.getByText("No recordings yet. Start a recording first!")).toBeVisible();
  });
});

test("renames a recording", async ({ page }) => {
  await page.goto("/recordings");
  await page.locator(".recording-item", { hasText: "Sprint planning" })
    .getByRole("button", { name: "Rename" })
    .click();
  const editor = page.locator(".rename-form");
  await editor.getByRole("textbox").fill("Planning session");
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(page.locator(".recording-item", { hasText: "Planning session" })).toBeVisible();
  await expect(page.getByText("Sprint planning", { exact: true })).toHaveCount(0);
});

test("deletes a recording after confirmation", async ({ page }) => {
  await page.goto("/recordings");
  page.once("dialog", async (dialog) => {
    expect(dialog.message()).toContain("interview.wav");
    await dialog.accept();
  });
  const item = page.locator(".recording-item", { hasText: "Customer interview" });
  await item.getByRole("button", { name: "Delete" }).click();
  await expect(page.getByText("Customer interview")).toHaveCount(0);
  await expect(page.getByText("Sprint planning")).toBeVisible();
});

test("opens the player for a saved audio file", async ({ page }) => {
  await page.goto("/recordings");
  const item = page.locator(".recording-item", { hasText: "Sprint planning" });
  await item.getByTitle("Play").click();
  await expect(page.locator(".ap-title")).toHaveText("Sprint planning");
  await expect(page.getByTitle("Close player")).toBeVisible();
});

test("opens a transcript without sending audio to a speech service", async ({ page }) => {
  const requests: { url: string; method: string; body: string | null }[] = [];
  page.on("request", (request) => {
    requests.push({ url: request.url(), method: request.method(), body: request.postData() });
  });

  await page.goto("/recordings");
  const item = page.locator(".recording-item", { hasText: "Customer interview" });
  await item.getByRole("button", { name: "Transcribe" }).click();
  await expect(page).toHaveURL(/\/transcription\/interview\.wav$/);
  await expect(page.getByRole("heading", { name: "Customer interview" })).toBeVisible();
  await expect(page.getByText("Thanks for joining.")).toBeVisible();

  const transcribe = requests.filter((request) =>
    request.method === "POST" && request.url.endsWith("/api/recordings/interview.wav/transcribe"));
  expect(transcribe).toHaveLength(1);
  expect(transcribe[0].body ?? "").not.toMatch(/RIFF|audio_base_64|data:audio/i);

  const appHosts = new Set(["127.0.0.1:5173", "localhost:5173", "127.0.0.1:8000", "localhost:8000"]);
  const outside = requests.map((request) => new URL(request.url).host).filter((host) => !appHosts.has(host));
  expect(outside).toEqual([]);
});
