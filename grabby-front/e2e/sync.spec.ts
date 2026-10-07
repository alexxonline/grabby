import { test, expect } from "./fixtures";
import type { SyncConflict } from "../src/api";

const enabled = { enabled: true, endpoint: "https://sync.example/exchange", api_key_configured: true };
const conflict: SyncConflict = {
  recording_id: "recording-123",
  local: { filename: "planning.mp3", title: "Local planning", deleted_at: null, transcription: { text: "Local transcript" }, notes: { text: "Local notes" } },
  server: { filename: "planning.mp3", title: "Server planning", deleted_at: null, transcription: { text: "Server transcript" }, notes: { text: "Server notes" } },
};

const otherConflict: SyncConflict = {
  recording_id: "recording-456",
  local: { filename: "standup.mp3", title: "Local standup", deleted_at: null, transcription: { text: "Unchanged local standup" }, notes: { text: "Unchanged local standup notes" } },
  server: { filename: "standup.mp3", title: "Server standup", deleted_at: null, transcription: { text: "Unchanged server standup" }, notes: { text: "Unchanged server standup notes" } },
};

const panel = (page: import("@playwright/test").Page) => page.locator("section").filter({ has: page.getByRole("heading", { name: "Text sync", exact: true }) });

test("sync starts disabled and unsaved changes cannot trigger an exchange", async ({ page }) => {
  await page.goto("/settings");
  const sync = panel(page);
  await expect(sync.getByLabel("Enable sync")).not.toBeChecked();
  await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
  await expect(sync.getByText("Last successful sync: Never")).toBeVisible();
  await expect(sync.getByLabel("API key")).toHaveAttribute("type", "password");
  await sync.getByLabel("Enable sync").check();
  await expect(sync.getByText("Save settings before syncing.")).toBeVisible();
  await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
});

test("saves HTTPS credentials, clears the key field and persists enabled sync", async ({ page }) => {
  await page.goto("/settings");
  const sync = panel(page);
  await sync.getByLabel("Enable sync").check();
  await sync.getByLabel("Receiving endpoint").fill(enabled.endpoint);
  await sync.getByLabel("API key").fill("test-secret");
  const sent = page.waitForRequest(r => r.url().endsWith("/api/sync/settings") && r.method() === "PUT");
  await sync.getByRole("button", { name: "Save sync settings" }).click();
  expect((await sent).postDataJSON()).toEqual({ enabled: true, endpoint: enabled.endpoint, mock: false, api_key: "test-secret" });
  await expect(sync.getByRole("status")).toHaveText("Sync settings saved.");
  await expect(sync.getByLabel("API key")).toHaveValue("");
  await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeEnabled();
  await page.reload();
  await expect(sync.getByLabel("Enable sync")).toBeChecked();
  await expect(sync.getByLabel("Receiving endpoint")).toHaveValue(enabled.endpoint);
  await expect(sync.getByLabel("API key")).toHaveValue("");
  await expect(sync.getByLabel("API key")).toHaveAttribute("placeholder", "Key saved; leave blank to keep it");
});

for (const invalid of [
  { endpoint: "", key: "", error: "An endpoint and API key are required" },
  { endpoint: "https://sync.example/exchange", key: "", error: "An endpoint and API key are required" },
  { endpoint: "http://sync.example/exchange", key: "test-key", error: "Use an HTTPS endpoint" },
]) {
  test(`rejects enabling sync with endpoint '${invalid.endpoint}' and ${invalid.key ? "a key" : "no key"}`, async ({ page }) => {
    await page.goto("/settings");
    const sync = panel(page);
    await sync.getByLabel("Enable sync").check();
    await sync.getByLabel("Receiving endpoint").fill(invalid.endpoint);
    await sync.getByLabel("API key").fill(invalid.key);
    await sync.getByRole("button", { name: "Save sync settings" }).click();
    await expect(sync.getByRole("alert")).toContainText(invalid.error);
    await expect(sync.getByLabel("Enable sync")).toBeChecked();
    await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
  });
}

test.describe("configured server", () => {
  test.use({ mockApi: { sync: enabled } });
  test("keeps the saved key when disabling sync", async ({ page }) => {
    await page.goto("/settings");
    const sync = panel(page);
    await sync.getByLabel("Enable sync").uncheck();
    const sent = page.waitForRequest(r => r.url().endsWith("/api/sync/settings") && r.method() === "PUT");
    await sync.getByRole("button", { name: "Save sync settings" }).click();
    expect((await sent).postDataJSON()).toEqual({ enabled: false, endpoint: enabled.endpoint, mock: false });
    await expect(sync.getByRole("status")).toHaveText("Sync settings saved.");
    await page.reload();
    await expect(sync.getByLabel("Enable sync")).not.toBeChecked();
    await expect(sync.getByLabel("API key")).toHaveAttribute("placeholder", "Key saved; leave blank to keep it");
    await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
  });

  test("shows an in-flight exchange and the last successful sync", async ({ page }) => {
    let complete!: () => void;
    const gate = new Promise<void>(resolve => { complete = resolve; });
    await page.route("**/api/sync/run", async route => { await gate; await route.fallback(); });
    await page.goto("/settings");
    const sync = panel(page);
    const requested = page.waitForRequest(r => r.url().endsWith("/api/sync/run"));
    await sync.getByRole("button", { name: "Sync now", exact: true }).click();
    const request = await requested;
    expect(request.method()).toBe("POST");
    expect(request.postData()).toBeNull();
    await expect(sync.getByRole("status")).toHaveText("Working…");
    await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
    await expect(sync.getByRole("button", { name: "Save sync settings" })).toBeDisabled();
    complete();
    await expect(sync.getByRole("status")).toHaveText("Sync completed.");
    await expect(sync.getByText("Last successful sync:")).not.toContainText("Never");
    await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeEnabled();
  });

  test("polls sync status without overwriting unsaved endpoint edits", async ({ page }) => {
    await page.clock.install();
    await page.goto("/settings");
    const sync = panel(page);
    await sync.getByLabel("Receiving endpoint").fill("https://new.example/sync");
    await page.route("**/api/sync/settings", route => route.fulfill({ json: { ...enabled, mock: false, device_id: "test-device", running: true, conflicts: 0, last_success: null, error: null } }));
    const polled = page.waitForResponse(r => r.url().endsWith("/api/sync/settings"));
    await page.clock.runFor(5000);
    await polled;
    await expect(sync.getByRole("status")).toHaveText("Working…");
    await expect(sync.getByLabel("Receiving endpoint")).toHaveValue("https://new.example/sync");
    await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
  });
});

test.describe("local mock", () => {
  test.use({ mockApi: { sync: { ...enabled, endpoint: "", mock: true } } });
  for (const [scenario, label] of [
    ["remote_edit", "Simulate another device"], ["conflict", "Simulate conflicting edits"],
    ["failure", "Simulate server failure"], ["authentication", "Simulate rejected key"],
  ]) {
    test(`sends the ${scenario} scenario and can run mock sync`, async ({ page }) => {
      await page.goto("/settings");
      const sync = panel(page);
      await expect(sync.getByLabel("Receiving endpoint")).toBeDisabled();
      const sent = page.waitForRequest(r => r.url().endsWith("/api/sync/mock/scenario"));
      await sync.getByRole("button", { name: label, exact: true }).click();
      expect((await sent).postDataJSON()).toEqual({ scenario });
      await expect(sync.getByRole("status")).toContainText("Mock scenario prepared.");
      await sync.getByRole("button", { name: "Test mock sync", exact: true }).click();
      if (scenario === "failure" || scenario === "authentication") {
        await expect(sync.getByRole("alert")).toHaveText(scenario === "failure" ? "Server unavailable" : "Invalid API key");
        await expect(sync.getByRole("button", { name: "Test mock sync", exact: true })).toBeEnabled();
        await sync.getByRole("button", { name: "Test mock sync", exact: true }).click();
        await expect(sync.getByRole("alert")).toHaveCount(0);
      }
      await expect(sync.getByRole("status")).toHaveText("Sync completed.");
      if (scenario === "conflict") {
        const article = sync.getByRole("article");
        await expect(article).toHaveCount(1);
        await expect(article.getByText("Alice: Local transcript", { exact: true })).toBeVisible();
        await expect(article.getByText("Alice: Remote transcript update", { exact: true })).toBeVisible();
        await expect(article.getByText("Notes: Local notes", { exact: true })).toBeVisible();
        await expect(article.getByText("Notes: Remote notes update", { exact: true })).toBeVisible();
        await page.reload();
        await expect(article).toHaveCount(1);
        // Receiving a conflicting version must not silently replace local text.
        await page.goto("/transcription/planning.mp3");
        await expect(page.getByText("Alice: Local transcript", { exact: true })).toBeVisible();
        await expect(page.getByText("Alice: Remote transcript update", { exact: true })).toHaveCount(0);
      } else if (scenario === "remote_edit") {
        await expect(sync.getByRole("article")).toHaveCount(0);
        await page.goto("/recordings");
        await expect(page.locator(".recording-item", { hasText: "Planning from another device" })).toBeVisible();
        await expect(page.getByText("Sprint planning", { exact: true })).toHaveCount(0);
        await page.goto("/transcription/planning.mp3");
        await expect(page.getByText("Alice: Remote transcript update", { exact: true })).toBeVisible();
        await expect(page.getByLabel("Your notes")).toHaveValue("Remote notes update");
      }
    });
  }
});

test("switches to mock mode and saves a test key without a remote endpoint", async ({ page }) => {
  await page.goto("/settings");
  const sync = panel(page);
  await sync.getByLabel("Use local mock server for testing").check();
  await sync.getByLabel("Enable sync").check();
  await sync.getByLabel("API key").fill("mock-key");
  await expect(sync.getByLabel("Receiving endpoint")).toBeDisabled();
  await expect(sync.getByRole("button", { name: "Simulate another device" })).toHaveCount(0);
  await sync.getByRole("button", { name: "Save sync settings" }).click();
  await expect(sync.getByRole("button", { name: "Test mock sync", exact: true })).toBeEnabled();
  await expect(sync.getByRole("button", { name: "Simulate another device" })).toBeVisible();
});

for (const choice of ["local", "server"] as const) {
  test.describe(`resolve with ${choice}`, () => {
    test.use({ mockApi: { sync: enabled, syncConflicts: [conflict, otherConflict] } });
    test("shows both text versions and resolves only the chosen record", async ({ page }) => {
      await page.goto("/settings");
      const sync = panel(page);
      await expect(sync.getByRole("article")).toHaveCount(2);
      const article = sync.getByRole("article").filter({ hasText: "Conflicting edits: planning.mp3" });
      const untouched = sync.getByRole("article").filter({ hasText: "Conflicting edits: standup.mp3" });
      const untouchedBefore = await untouched.innerText();
      for (const text of ["Local transcript", "Server transcript", "Notes: Local notes", "Notes: Server notes"]) await expect(article.getByText(text, { exact: true })).toBeVisible();
      const sent = page.waitForRequest(r => r.url().endsWith("/api/sync/conflicts/recording-123") && r.method() === "POST");
      await article.getByRole("button", { name: choice === "local" ? "Keep this device" : "Keep server", exact: true }).click();
      expect((await sent).postDataJSON()).toEqual({ choice });
      await expect(article).toHaveCount(0);
      await expect(sync.getByRole("article")).toHaveCount(1);
      await expect(untouched).toHaveText(untouchedBefore, { useInnerText: true });
      await expect(sync.getByRole("status")).toHaveText(choice === "local" ? "Local version kept. Sync to upload it." : "Server version applied.");
      await page.reload();
      await expect(article).toHaveCount(0);
      await expect(sync.getByRole("article")).toHaveCount(1);
      await expect(untouched).toHaveText(untouchedBefore, { useInnerText: true });
      await page.goto("/transcription/planning.mp3");
      await expect(page.getByText(choice === "local" ? "Local transcript" : "Server transcript", { exact: true })).toBeVisible();
      await expect(page.getByLabel("Your notes")).toHaveValue(choice === "local" ? "Local notes" : "Server notes");
    });
  });
}

test.describe("load failure", () => {
  test.use({ mockApi: { syncLoadFails: true } });
  test("shows the settings error", async ({ page }) => {
    await page.goto("/settings");
    await expect(panel(page).getByRole("alert")).toHaveText("Sync settings unavailable");
    await expect(panel(page).getByRole("button", { name: "Save sync settings" })).toHaveCount(0);
  });
});

test.describe("save failure", () => {
  test.use({ mockApi: { sync: enabled, syncSaveFails: true } });
  test("keeps edits and allows another save attempt", async ({ page }) => {
    await page.goto("/settings");
    const sync = panel(page);
    await sync.getByLabel("Receiving endpoint").fill("https://new.example/sync");
    await sync.getByRole("button", { name: "Save sync settings" }).click();
    await expect(sync.getByRole("alert")).toHaveText("Could not save sync settings");
    await expect(sync.getByLabel("Receiving endpoint")).toHaveValue("https://new.example/sync");
    await expect(sync.getByRole("button", { name: "Save sync settings" })).toBeEnabled();
    await expect(sync.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled();
  });
});
