import Router, { route } from "preact-router";
import { useEffect } from "preact/hooks";
import { RecorderPage } from "./pages/RecorderPage";
import { RecordingsPage } from "./pages/RecordingsPage";
import { TranscriptionPage } from "./pages/TranscriptionPage";
import { SearchPage } from "./pages/SearchPage";
import { SettingsPage } from "./pages/SettingsPage";
import "./app.css";

function RedirectHome(_props: { default?: boolean }) {
  useEffect(() => {
    route("/", true);
  }, []);
  return null;
}

function Nav() {
  return (
    <nav class="navbar">
      <div class="nav-brand">Grabby</div>
      <div class="nav-links">
        <a href="/" class="nav-link">Recorder</a>
        <a href="/recordings" class="nav-link">Recordings</a>
        <a href="/search" class="nav-link">Search</a>
        <a href="/settings" class="nav-link">Settings</a>
      </div>
    </nav>
  );
}

export function App() {
  return (
    <>
      <Nav />
      <main class="container">
        <Router>
          <RecorderPage path="/" />
          <RecordingsPage path="/recordings" />
          <SearchPage path="/search" />
          <SettingsPage path="/settings" />
          <TranscriptionPage path="/transcription/:filename" />
          <RedirectHome default />
        </Router>
      </main>
    </>
  );
}
