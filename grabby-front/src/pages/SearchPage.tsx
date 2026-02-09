import { useState } from "preact/hooks";
import { api } from "../api";
import type { SearchResult } from "../api";

interface Props {
  path?: string;
}

export function SearchPage(_props: Props) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [searched, setSearched] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const doSearch = async () => {
    const q = query.trim();
    if (!q) return;
    setLoading(true);
    setError(null);
    try {
      const data = await api.searchTranscriptions(q);
      setResults(data);
      setSearched(true);
    } catch (err: any) {
      setError(err.message || "Search failed");
    } finally {
      setLoading(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") doSearch();
  };

  return (
    <div class="page search-page">
      <h1>Search Transcripts</h1>

      <div class="search-bar">
        <input
          type="text"
          class="search-input"
          placeholder="Search transcripts…"
          value={query}
          onInput={(e) => setQuery((e.target as HTMLInputElement).value)}
          onKeyDown={onKeyDown}
        />
        <button class="btn btn-search" onClick={doSearch} disabled={loading || !query.trim()}>
          {loading ? "Searching…" : "Search"}
        </button>
      </div>

      {error && <p class="error">{error}</p>}

      {searched && !loading && results.length === 0 && (
        <p class="empty">No results found for "{query}"</p>
      )}

      {results.length > 0 && (
        <ul class="search-results">
          {results.map((r) => (
            <li key={r.filename} class="search-result-item">
              <a href={`/transcription/${encodeURIComponent(r.filename)}`} class="search-result-link">
                <span class="search-result-name">{r.filename.replace(/\.[^.]+$/, "")}</span>
                <span
                  class="search-result-snippet"
                  dangerouslySetInnerHTML={{ __html: r.snippet.replace(/\*\*(.*?)\*\*/g, "<mark>$1</mark>") }}
                />
              </a>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
