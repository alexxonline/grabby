import assert from "node:assert/strict";
import test from "node:test";
import { renderSearchSnippet } from "../src/components/searchSnippet.ts";

test("transcript HTML remains text, including highlighted HTML", () => {
  const payload = '<img src=x onerror=alert(1)>';
  const parts = renderSearchSnippet(`${payload} **${payload}** & <script>alert(1)</script>`);
  assert.equal(parts[0], `${payload} `);
  assert.equal(parts[1].type, "mark");
  assert.equal(parts[1].props.children, payload);
  assert.equal(parts[1].props.dangerouslySetInnerHTML, undefined);
  assert.equal(parts[2], " & <script>alert(1)</script>");
});

test("search highlights preserve text and multiple matches", () => {
  const parts = renderSearchSnippet("Before **first** and **second** after");
  assert.deepEqual(parts.filter(part => typeof part === "string"), ["Before ", " and ", " after"]);
  assert.deepEqual(parts.filter(part => typeof part !== "string").map(part => part.props.children), ["first", "second"]);
  assert.deepEqual(renderSearchSnippet("Unmatched ** marker"), ["Unmatched ** marker"]);
});
