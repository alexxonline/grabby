import { test as base, expect } from "@playwright/test";
import { installGrabbyApi, type MockOptions } from "./mockApi";

type Fixtures = {
  mockApi: MockOptions;
};

export const test = base.extend<Fixtures>({
  mockApi: [{}, { option: true }],
  page: async ({ page, mockApi }, use) => {
    await installGrabbyApi(page, mockApi);
    await use(page);
  },
});

export { expect };
