"""TTL cache for deterministic agent results, optionally persisted in SQLite."""

from __future__ import annotations

import asyncio
import hashlib
import json
import sqlite3
import time
from collections import OrderedDict
from pathlib import Path
from typing import Any

from .client import OllamaError


class ResponseCacheError(OllamaError):
    """The configured persistent response cache could not be read or written."""


class ResponseCache:
    def __init__(self, settings: dict[str, Any]) -> None:
        self.enabled = bool(settings.get("enabled", False))
        self.ttl_seconds = max(1, int(settings.get("ttl_seconds", 300)))
        self.max_entries = max(1, int(settings.get("max_entries", 256)))
        self.deterministic_agents = set(settings.get("deterministic_agents", ["automation", "summarize"]))
        configured_path = settings.get("sqlite_path")
        self.sqlite_path = Path(configured_path) if configured_path else None
        self._values: OrderedDict[str, tuple[float, Any]] = OrderedDict()
        self._lock = asyncio.Lock()

    @staticmethod
    def make_key(
        agent: str,
        prompt: str,
        history: list[dict[str, Any]],
        extra: Any = None,
    ) -> str:
        normalized_prompt = " ".join(prompt.casefold().split())
        payload = json.dumps(
            [agent, normalized_prompt, history, extra],
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ":"),
        )
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()

    async def get(self, key: str) -> Any | None:
        if not self.enabled:
            return None
        now = time.time()
        async with self._lock:
            cached = self._values.get(key)
            if cached is not None:
                expires_at, value = cached
                if expires_at > now:
                    self._values.move_to_end(key)
                    return value
                del self._values[key]
            if self.sqlite_path is None:
                return None
            try:
                row = await asyncio.to_thread(self._read_sqlite, key, now)
            except (sqlite3.Error, OSError, ValueError) as error:
                raise ResponseCacheError(f"Could not read response cache: {error}") from error
            if row is None:
                return None
            expires_at, value = row
            self._remember(key, expires_at, value)
            return value

    async def put(self, key: str, value: Any) -> None:
        if not self.enabled:
            return
        expires_at = time.time() + self.ttl_seconds
        encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        async with self._lock:
            self._remember(key, expires_at, value)
            if self.sqlite_path is None:
                return
            try:
                await asyncio.to_thread(self._write_sqlite, key, expires_at, encoded)
            except (sqlite3.Error, OSError) as error:
                raise ResponseCacheError(f"Could not persist response cache: {error}") from error

    def _remember(self, key: str, expires_at: float, value: Any) -> None:
        self._values[key] = (expires_at, value)
        self._values.move_to_end(key)
        while len(self._values) > self.max_entries:
            self._values.popitem(last=False)

    def _read_sqlite(self, key: str, now: float) -> tuple[float, Any] | None:
        assert self.sqlite_path is not None
        self.sqlite_path.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(self.sqlite_path)
        try:
            with connection:
                connection.execute(
                    "CREATE TABLE IF NOT EXISTS response_cache "
                    "(cache_key TEXT PRIMARY KEY, expires_at REAL NOT NULL, value TEXT NOT NULL)"
                )
                connection.execute(
                    "DELETE FROM response_cache WHERE expires_at <= ?",
                    (now,),
                )
                row = connection.execute(
                    "SELECT expires_at, value FROM response_cache "
                    "WHERE cache_key = ? AND expires_at > ?",
                    (key, now),
                ).fetchone()
        finally:
            connection.close()
        if row is None:
            return None
        expires_at, encoded = row
        return float(expires_at), json.loads(encoded)

    def _write_sqlite(self, key: str, expires_at: float, encoded: str) -> None:
        assert self.sqlite_path is not None
        self.sqlite_path.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(self.sqlite_path)
        try:
            with connection:
                connection.execute(
                    "CREATE TABLE IF NOT EXISTS response_cache "
                    "(cache_key TEXT PRIMARY KEY, expires_at REAL NOT NULL, value TEXT NOT NULL)"
                )
                connection.execute(
                    "INSERT INTO response_cache(cache_key, expires_at, value) "
                    "VALUES (?, ?, ?) ON CONFLICT(cache_key) DO UPDATE SET "
                    "expires_at = excluded.expires_at, value = excluded.value",
                    (key, expires_at, encoded),
                )
        finally:
            connection.close()
