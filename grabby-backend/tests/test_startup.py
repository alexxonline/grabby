"""Startup remains available when local storage or the optional frontend fails."""
import importlib.util
import sqlite3
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from fastapi.testclient import TestClient
from app import database as db
from app import main


class StartupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for item in (
            patch.object(db, "DB_PATH", self.root / "test.db"),
            patch.object(db, "_local", threading.local()),
            patch.object(main, "FRONTEND_DIST", self.root / "dist"),
            patch.object(main, "_database_ready", False),
            patch.object(main, "_database_retry_at", 0.0),
        ):
            item.start()
            self.addCleanup(item.stop)

    def test_import_does_not_initialize_storage(self):
        spec = importlib.util.spec_from_file_location("app._startup_probe", main.__file__)
        module = importlib.util.module_from_spec(spec)
        with patch.object(db, "init_db", side_effect=sqlite3.OperationalError("unavailable")) as init, \
                patch.object(Path, "mkdir", side_effect=PermissionError("read only")):
            spec.loader.exec_module(module)
        init.assert_not_called()

    def test_failed_startup_returns_503_and_retries_without_restart(self):
        real_init = db.init_db
        with patch.object(db, "init_db", side_effect=sqlite3.OperationalError("locked")) as init, \
                self.assertLogs("app.main", level="ERROR"), TestClient(main.app) as client:
            self.assertEqual(client.get("/api/status").status_code, 200)
            health = client.get("/api/health")
            self.assertEqual(health.status_code, 503)
            self.assertEqual(health.json()["database"], "unavailable")
            response = client.get("/api/settings")
            self.assertEqual(response.status_code, 503)
            self.assertEqual(response.headers["retry-after"], "5")
            with patch.object(main.recorder, "start") as start:
                self.assertEqual(client.post("/api/start").status_code, 503)
                start.assert_not_called()
            self.assertEqual(init.call_count, 1)  # Polls respect retry backoff.
            init.side_effect = real_init
            with patch.object(main, "_database_retry_at", 0.0):
                self.assertEqual(client.get("/api/health").status_code, 200)
            with patch.object(db, "get_settings", return_value={"auto_title": False, "auto_delete_audio": False}):
                self.assertEqual(client.get("/api/settings").status_code, 200)

    def test_corrupt_database_does_not_prevent_startup_or_get_replaced(self):
        original = b"this is not a sqlite database"
        db.DB_PATH.write_bytes(original)
        with self.assertLogs("app.main", level="ERROR"), TestClient(main.app) as client:
            self.assertEqual(client.get("/api/health").status_code, 503)
            self.assertEqual(client.get("/api/status").status_code, 200)
        self.assertEqual(db.DB_PATH.read_bytes(), original)

    def test_inaccessible_database_path_can_recover(self):
        with patch.object(db, "DB_PATH", self.root / "missing" / "test.db"), \
                self.assertLogs("app.main", level="ERROR"), TestClient(main.app) as client:
            self.assertEqual(client.get("/api/health").status_code, 503)
            db.DB_PATH.parent.mkdir()
            with patch.object(main, "_database_retry_at", 0.0):
                self.assertEqual(client.get("/api/health").status_code, 200)

    def test_missing_and_partial_frontend_do_not_block_api(self):
        with TestClient(main.app) as client:
            self.assertEqual(client.get("/").status_code, 404)
            self.assertEqual(client.get("/api/health").status_code, 200)
            main.FRONTEND_DIST.mkdir()
            self.assertEqual(client.get("/assets/missing.js").status_code, 404)
            (main.FRONTEND_DIST / "index.html").write_text("<html>Grabby</html>")
            self.assertEqual(client.get("/meetings").text, "<html>Grabby</html>")
            self.assertEqual(client.get("/assets/missing.js").status_code, 404)
            assets = main.FRONTEND_DIST / "assets"
            assets.mkdir()
            (assets / "app.js").write_text("console.log('ready')")
            self.assertEqual(client.get("/assets/app.js").text, "console.log('ready')")
            self.assertEqual(client.get("/api/unknown").status_code, 404)
