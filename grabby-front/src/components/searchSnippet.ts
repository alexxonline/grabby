import { h } from "preact";

export function renderSearchSnippet(snippet: string) {
  return snippet.split(/(\*\*.*?\*\*)/g).map((part, index) =>
    part.startsWith("**") && part.endsWith("**")
      ? h("mark", { key: index }, part.slice(2, -2))
      : part
  );
}
