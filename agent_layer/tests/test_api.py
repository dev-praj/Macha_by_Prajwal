from __future__ import annotations

import json
import unittest
from collections.abc import AsyncIterator
from typing import Any
from unittest.mock import patch

import httpx

from agent_layer.api import create_app, load_config, openai_messages, response_format_options
from agent_layer.client import OllamaError
from agent_layer.orchestrator import StreamEvent


class FakeClient:
    async def list_models(self) -> list[dict[str, Any]]:
        return []

    async def loaded_models(self) -> list[dict[str, Any]]:
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

    async def test_chat_passes_conversation_id_to_memory_layer(self) -> None:
        await self.client.post(
            "/chat",
            json={"prompt": "Hello", "conversation_id": "session-1"},
        )
        self.assertEqual(self.orchestrator.calls[0]["conversation_id"], "session-1")

    async def test_openai_stream_emits_compatible_chunks_and_done_marker(self) -> None:
        response = await self.client.post("/v1/chat/completions", json={
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": True,
            "model": "test-model",
        })
        self.assertEqual(response.status_code, 200)
        records = [line[6:] for line in response.text.splitlines() if line.startswith("data: {")]
        chunk = json.loads(records[0])
        self.assertEqual(chunk["object"], "chat.completion.chunk")
        self.assertEqual(chunk["model"], "test-model")
        self.assertEqual(chunk["choices"][0]["delta"]["content"], "Hello")
        self.assertIn("data: [DONE]", response.text)

    async def test_openai_non_streaming_completion_and_generation_options(self) -> None:
        response = await self.client.post("/v1/chat/completions", json={
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": False,
            "temperature": 0.2,
            "max_tokens": 48,
        })
        self.assertEqual(response.status_code, 200)
        body = response.json()
        self.assertEqual(body["object"], "chat.completion")
        self.assertEqual(body["choices"][0]["message"]["content"], "Hello")
        self.assertEqual(
            self.orchestrator.calls[0]["generation_options"],
            {"temperature": 0.2, "num_predict": 48},
        )

    async def test_openai_preserves_conversation_history_and_schema_format(self) -> None:
        prompt, history = openai_messages([
            {"role": "system", "content": "Be concise"},
            {"role": "user", "content": "Earlier"},
            {"role": "assistant", "content": "Answer"},
            {"role": "user", "content": "Now"},
        ])
        self.assertEqual(prompt, "Now")
        self.assertEqual([item["role"] for item in history], ["system", "user", "assistant"])
        task, schema = response_format_options({
            "type": "json_schema",
            "json_schema": {
                "name": "action",
                "schema": {"type": "object", "properties": {"action": {"type": "string"}}},
            },
        })
        self.assertEqual(task, "automation")
        self.assertEqual(schema["type"], "object")

    async def test_openai_endpoint_forwards_history_and_json_schema(self) -> None:
        response = await self.client.post("/v1/chat/completions", json={
            "messages": [
                {"role": "user", "content": "Earlier"},
                {"role": "assistant", "content": "Prior answer"},
                {"role": "user", "content": "Now"},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "result",
                    "schema": {
                        "type": "object",
                        "properties": {"result": {"type": "string"}},
                        "required": ["result"],
                    },
                },
            },
        })
        self.assertEqual(response.status_code, 200)
        call = self.orchestrator.calls[0]
        self.assertEqual(call["task"], "automation")
        self.assertEqual([message["role"] for message in call["history"]], ["user", "assistant"])
        self.assertEqual(call["tool_schema"]["required"], ["result"])

    async def test_openai_rejects_invalid_messages_and_response_formats(self) -> None:
        response = await self.client.post("/v1/chat/completions", json={
            "messages": [{"role": "assistant", "content": "No final user"}],
        })
        self.assertEqual(response.status_code, 400)
        with self.assertRaisesRegex(ValueError, "top-level object schema"):
            response_format_options({
                "type": "json_schema",
                "json_schema": {"schema": {"type": "array"}},
            })

    async def test_openai_stream_emits_upstream_errors(self) -> None:
        self.app.state.orchestrator = FakeOrchestrator(error=OllamaError("unavailable"))
        response = await self.client.post("/v1/chat/completions", json={
            "messages": [{"role": "user", "content": "Hello"}],
        })
        self.assertIn('"error": "unavailable"', response.text)
        self.assertIn("data: [DONE]", response.text)

    async def test_app_lifespan_runs_runtime_initialization(self) -> None:
        with patch("agent_layer.runtime._ollama_ps", return_value=([], {})) as ps:
            async with self.app.router.lifespan_context(self.app):
                response = await self.client.get("/health")
        self.assertEqual(response.status_code, 200)
        ps.assert_awaited_once()


if __name__ == "__main__":
    unittest.main()
