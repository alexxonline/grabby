import assert from "node:assert/strict";
import test from "node:test";
import { api } from "../src/api.ts";

test("large Unicode notes use ordinary fetch and round-trip without truncation", async (t) => {
  const text = "Meeting notes: 📝\n".repeat(10000);
  t.mock.method(globalThis, "fetch", async (url, options) => {
    const size = new TextEncoder().encode(options.body).length;
    if (options.keepalive && size > 65536) throw new TypeError("keepalive quota exceeded");
    assert.ok(size > 65536);
    assert.equal(JSON.parse(options.body).text, text);
    return Response.json({ id: "notes", text });
  });
  assert.deepEqual(await api.saveNotes("notes", text), { id: "notes", text });
});
