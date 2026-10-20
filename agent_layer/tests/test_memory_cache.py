from __future__ import annotations

import sqlite3
import tempfile
import unittest
from pathlib import Path
from typing import Any

from agent_layer.agents import create_agents
from agent_layer.api import load_config
from agent_layer.cache import ResponseCache, ResponseCacheError
from agent_layer.client import OllamaError, StreamResult
from agent_layer.memory import ConversationMemory


class FakeClient:
    def __init__(self) -> None:
        self.loaded: list[dict[str, Any]] = []
        self.calls: list[dict[str, Any]] = []

    async def loaded_models(self) -> list[dict[str, Any]]:
        return self.loaded

    async def stream_chat(self, **kwargs: Any) -> StreamResult:
        self.calls.append(kwargs)
        return StreamResult("older facts retained", 0.01, 0.1, 1, 2, 0, 0.05, {})


class MemoryCacheTests(unittest.IsolatedAsyncioTestCase):
    async def test_memory_compresses_old_turns_and_keeps_recent_verbatim(self) -> None:
        config = load_config()
        config["memory"].update(enabled=True, last_n_turns=1, token_budget=80)
        client = FakeClient()
        summary_model = config["profiles"]["fast"]["models"]["summarize"]
        client.loaded = [{"name": summary_model}]
        summarizer = create_agents(config, client)["summarize"]  # type: ignore[arg-type]
        memory = ConversationMemory(config, client, summarizer)
        history = [
            {"role": "user", "content": "old question " * 30},
            {"role": "assistant", "content": "old answer " * 30},
            {"role": "user", "content": "middle question " * 20},
            {"role": "assistant", "content": "middle answer " * 20},
            {"role": "user", "content": "last question"},
            {"role": "assistant", "content": "last answer"},
        ]

        bounded = await memory.prepare("current request", history, "conversation-a")

        self.assertEqual(bounded[-2:], history[-2:])
        self.assertIn("older facts retained", bounded[-3]["content"])
        self.assertEqual(len(client.calls), 1)
        self.assertLessEqual(
            memory._token_count(
                "current request",
                bounded,
                config["agents"]["chat"]["system"],
            ),
            config["memory"]["token_budget"],
        )

    async def test_memory_refuses_to_load_missing_summarizer(self) -> None:
        config = load_config()
        config["memory"].update(enabled=True, last_n_turns=1, token_budget=80)
        client = FakeClient()
        summarizer = create_agents(config, client)["summarize"]  # type: ignore[arg-type]
        memory = ConversationMemory(config, client, summarizer)
        history = [
            {"role": "user", "content": "old question " * 30},
            {"role": "assistant", "content": "old answer " * 30},
            {"role": "user", "content": "last question"},
        ]

        with self.assertRaisesRegex(OllamaError, "already-loaded summarizer"):
            await memory.prepare("current request", history)

    async def test_memory_refuses_to_load_summarizer_alongside_heavy_model(self) -> None:
        config = load_config()
        config["memory"].update(enabled=True, last_n_turns=1, token_budget=80)
        client = FakeClient()
        client.loaded = [
            {"name": config["profiles"]["fast"]["models"]["summarize"]},
            {"name": config["profiles"]["heavy"]["model"]},
        ]
        summarizer = create_agents(config, client)["summarize"]  # type: ignore[arg-type]
        memory = ConversationMemory(config, client, summarizer)
        history = [
            {"role": "user", "content": "old question " * 30},
            {"role": "assistant", "content": "old answer " * 30},
            {"role": "user", "content": "last question"},
        ]

        with self.assertRaisesRegex(OllamaError, "heavy model is loaded"):
            await memory.prepare("current request", history)

    async def test_memory_keeps_rolling_summary_for_incremental_turns(self) -> None:
        config = load_config()
        config["memory"].update(enabled=True, last_n_turns=1, token_budget=40)
        client = FakeClient()
        summary_model = config["profiles"]["fast"]["models"]["summarize"]
        client.loaded = [{"name": summary_model}]
        summarizer = create_agents(config, client)["summarize"]  # type: ignore[arg-type]
        memory = ConversationMemory(config, client, summarizer)
        history = [
            {"role": "user", "content": "question " * 30},
            {"role": "assistant", "content": "answer " * 30},
            {"role": "user", "content": "recent one"},
            {"role": "assistant", "content": "response one"},
        ]
        await memory.prepare("request one", history, "rolling")
        history.extend([
            {"role": "user", "content": "recent two"},
            {"role": "assistant", "content": "response two"},
        ])
        await memory.prepare("request two", history, "rolling")

        self.assertEqual(len(client.calls), 2)
        self.assertIn("Update this rolling conversation summary", client.calls[1]["messages"][1]["content"])

    async def test_memory_budget_does_not_silently_truncate_without_old_turns(self) -> None:
        config = load_config()
        config["memory"].update(enabled=True, last_n_turns=2, token_budget=2)
        client = FakeClient()
        summarizer = create_agents(config, client)["summarize"]  # type: ignore[arg-type]
        memory = ConversationMemory(config, client, summarizer)

        with self.assertRaisesRegex(OllamaError, "no older complete turns"):
            await memory.prepare(
                "long current prompt",
                [{"role": "user", "content": "one turn"}],
            )

    async def test_cache_uses_ttl_eviction_and_optional_sqlite_persistence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cache.sqlite"
            settings = {
                "enabled": True,
                "ttl_seconds": 300,
                "max_entries": 1,
                "sqlite_path": str(path),
                "deterministic_agents": ["automation", "summarize"],
            }
            cache = ResponseCache(settings)
            key = cache.make_key("automation", "Open   App", [], {"type": "object"})
            self.assertEqual(
                key,
                cache.make_key("automation", "open app", [], {"type": "object"}),
            )
            await cache.put(key, {"action": "open_app"})
            self.assertEqual(await cache.get(key), {"action": "open_app"})

            reopened = ResponseCache(settings)
            self.assertEqual(await reopened.get(key), {"action": "open_app"})
            self.assertTrue(path.exists())

    async def test_cache_surfaces_invalid_persistent_value(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cache.sqlite"
            connection = sqlite3.connect(path)
            try:
                connection.execute(
                    "CREATE TABLE response_cache "
                    "(cache_key TEXT PRIMARY KEY, expires_at REAL NOT NULL, value TEXT NOT NULL)"
                )
                connection.execute(
                    "INSERT INTO response_cache VALUES ('broken', 9999999999, '{invalid')"
                )
                connection.commit()
            finally:
                connection.close()
            cache = ResponseCache({
                "enabled": True,
                "sqlite_path": str(path),
            })
            with self.assertRaises(ResponseCacheError):
                await cache.get("broken")


if __name__ == "__main__":
    unittest.main()
