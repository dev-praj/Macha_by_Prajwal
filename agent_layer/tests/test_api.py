from __future__ import annotations

import unittest
from collections.abc import AsyncIterator
from typing import Any

import httpx

from agent_layer.api import create_app, load_config
from agent_layer.client import OllamaError
from agent_layer.orchestrator import StreamEvent


class FakeClient:
    async def list_models(self) -> list[dict[str, Any]]:
        return []


class FakeOrchestrator:
    def __init__(self, events: list[StreamEvent] | None = None, error: Exception | None = None) -> None:
        self.events = events or [
            StreamEvent("meta", {"task": "chat", "agent": "chat"}),
            StreamEvent("token", {"content": "Hello"}),
            StreamEvent("done", {"task": "chat", "agent": "chat"}),
        ]
        self.error = error
        self.calls: list[dict[str, Any]] = []

    async def stream(self, prompt: str, **kwargs: Any) -> AsyncIterator[StreamEvent]:
        self.calls.append({"prompt": prompt, **kwargs})
        for event in self.events:
            yield event
        if self.error:
            raise self.error


class AgentApiTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self.app = create_app(load_config(), FakeClient())  # type: ignore[arg-type]
        self.orchestrator = FakeOrchestrator()
        self.app.state.orchestrator = self.orchestrator
        self.client = httpx.AsyncClient(
            transport=httpx.ASGITransport(app=self.app),
            base_url="http://test",
        )

    async def asyncTearDown(self) -> None:
        await self.client.aclose()

    async def test_chat_sse_stream_includes_metadata_tokens_and_done(self) -> None:
        response = await self.client.post("/chat", json={"prompt": "Hello"})
        self.assertEqual(response.status_code, 200)
        self.assertIn("event: meta", response.text)
        self.assertIn('"content": "Hello"', response.text)
        self.assertIn("event: done", response.text)

    async def test_chat_stream_surfaces_ollama_error_as_sse_event(self) -> None:
        self.app.state.orchestrator = FakeOrchestrator(error=OllamaError("unavailable"))
        response = await self.client.post("/chat", json={"prompt": "Hello"})
        self.assertIn("event: error", response.text)
        self.assertIn("unavailable", response.text)


if __name__ == "__main__":
    unittest.main()
