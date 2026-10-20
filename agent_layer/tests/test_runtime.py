from __future__ import annotations

import unittest
from typing import Any
from unittest.mock import patch

from agent_layer.api import load_config
from agent_layer.client import StreamResult
from agent_layer.runtime import GIB, initialize_runtime


class FakeClient:
    def __init__(self, installed: list[dict[str, Any]], loaded: list[dict[str, Any]]) -> None:
        self.installed = installed
        self.loaded = loaded
        self.calls: list[dict[str, Any]] = []

    async def list_models(self) -> list[dict[str, Any]]:
        return self.installed

    async def loaded_models(self) -> list[dict[str, Any]]:
        return self.loaded

    async def stream_chat(self, **kwargs: Any) -> StreamResult:
        self.calls.append(kwargs)
        return StreamResult("ok", 0.01, 0.02, 1, 1, 0, 0.01, {})


class RuntimeTests(unittest.IsolatedAsyncioTestCase):
    @patch("agent_layer.runtime._ollama_ps", return_value=([], {}))
    async def test_startup_warms_installed_fast_model_and_holds_keep_alive(
        self,
        _ps: object,
    ) -> None:
        config = load_config()
        config["profiles"]["fast"]["models"] = {"chat": "qwen2.5:7b"}
        client = FakeClient([{"name": "qwen2.5:7b", "size": GIB}], [])

        await initialize_runtime(config, client)  # type: ignore[arg-type]

        self.assertEqual(len(client.calls), 1)
        self.assertEqual(client.calls[0]["model"], "qwen2.5:7b")
        self.assertEqual(client.calls[0]["keep_alive"], -1)
        self.assertEqual(client.calls[0]["options"]["num_predict"], 1)
        self.assertEqual(client.calls[0]["options"]["num_ctx"], config["agents"]["chat"]["num_ctx"])
        self.assertEqual(
            client.calls[0]["messages"][0]["content"],
            config["agents"]["chat"]["system"],
        )

    @patch("agent_layer.runtime._ollama_ps", return_value=([], {}))
    async def test_startup_skips_warmup_when_fast_profile_exceeds_budget(
        self,
        _ps: object,
    ) -> None:
        config = load_config()
        config["profiles"]["fast"]["models"] = {"chat": "large-fast-model"}
        config["optimizations"]["memory_budget_gb"] = 16
        client = FakeClient([{"name": "large-fast-model", "size": 17 * GIB}], [])

        await initialize_runtime(config, client)  # type: ignore[arg-type]

        self.assertEqual(client.calls, [])

    @patch("agent_layer.runtime._ollama_ps", return_value=([], {}))
    async def test_startup_does_not_warm_fast_profile_alongside_heavy_model(
        self,
        _ps: object,
    ) -> None:
        config = load_config()
        config["profiles"]["fast"]["models"] = {"chat": "qwen2.5:7b"}
        client = FakeClient(
            [{"name": "qwen2.5:7b", "size": GIB}],
            [{"name": "gpt-oss:20b"}],
        )

        await initialize_runtime(config, client)  # type: ignore[arg-type]

        self.assertEqual(client.calls, [])


if __name__ == "__main__":
    unittest.main()
