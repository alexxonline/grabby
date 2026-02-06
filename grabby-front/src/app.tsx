import Router from "preact-router";
import { RecorderPage } from "./pages/RecorderPage";
import { RecordingsPage } from "./pages/RecordingsPage";
import "./app.css";

function Nav() {
  return (
    <nav class="navbar">
      <div class="nav-brand">Grabby</div>
      <div class="nav-links">
        <a href="/" class="nav-link">Recorder</a>
        <a href="/recordings" class="nav-link">Recordings</a>
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
        </Router>
      </main>
    </>
  );
}
