from __future__ import annotations

import unittest
from collections.abc import AsyncIterator
from typing import Any

from agent_layer.agents import AutomationAgent, VoiceAgent, create_agents
from agent_layer.client import OllamaError, StreamResult
from agent_layer.api import load_config
from agent_layer.orchestrator import Orchestrator
from agent_layer.router import TaskRouter


class FakeClient:
    def __init__(self) -> None:
        self.calls: list[dict[str, Any]] = []
        self.outputs: list[str] = []
        self.installed: list[dict[str, Any]] = []
        self.loaded: list[dict[str, Any]] = []

    async def chat_stream(self, **kwargs: Any) -> AsyncIterator[str]:
        self.calls.append(kwargs)
        output = self.outputs.pop(0) if self.outputs else "ok"
        for part in output.split("|"):
            yield part

    async def stream_chat(self, **kwargs: Any) -> StreamResult:
        self.calls.append(kwargs)
        output = self.outputs.pop(0) if self.outputs else '{"task":"chat"}'
        return StreamResult(output, 0.01, 0.1, 1, 2, 0, 0.05, {})

    async def list_models(self) -> list[dict[str, Any]]:
        return self.installed

    async def loaded_models(self) -> list[dict[str, Any]]:
        return self.loaded


class AgentLayerTests(unittest.IsolatedAsyncioTestCase):
    async def test_router_rules_handle_explicit_and_specialized_requests(self) -> None:
        client = FakeClient()
        router = TaskRouter(load_config(), client)  # type: ignore[arg-type]

        self.assertEqual((await router.route("Write a Python function")).task, "code")
        self.assertEqual((await router.route("Open the calculator")).task, "automation")
        self.assertEqual((await router.route("Please summarize this text")).task, "summarize")
        self.assertEqual((await router.route("plain question", "voice")).source, "request")
        with self.assertRaisesRegex(ValueError, "Unsupported task"):
            await router.route("question", "unknown")

    async def test_router_uses_json_model_only_when_ambiguous_and_installed(self) -> None:
        config = load_config()
        model = config["profiles"]["fast"]["models"]["router"]
        client = FakeClient()
        client.installed = [{"name": model}]
        client.outputs = ['{"task":"code"}']
        route = await TaskRouter(config, client).route("Could this be useful?")

        self.assertEqual((route.task, route.source), ("code", "model"))
        self.assertEqual(client.calls[0]["response_format"]["type"], "object")

    async def test_router_falls_back_without_pulling_missing_router_model(self) -> None:
        client = FakeClient()
        route = await TaskRouter(load_config(), client).route("Can you help me?")
        self.assertEqual(route.task, "chat")
        self.assertEqual(route.source, "rules-fallback-router-model-missing")

    async def test_agent_options_and_system_prefix_come_from_config(self) -> None:
        config = load_config()
        client = FakeClient()
        agents = create_agents(config, client)  # type: ignore[arg-type]
        agent = agents["chat"]
        first = agent.messages("first", [])
        second = agent.messages("second", [{"role": "user", "content": "earlier"}])

        self.assertEqual(first[0]["content"], second[0]["content"])
        self.assertEqual(first[0]["content"], config["agents"]["chat"]["system"])
        self.assertEqual(agent.model_name(), config["profiles"]["fast"]["models"]["chat"])
        self.assertEqual(agent.options()["num_ctx"], config["agents"]["chat"]["num_ctx"])

    async def test_voice_agent_yields_sentences_as_the_stream_arrives(self) -> None:
        config = load_config()
        client = FakeClient()
        client.outputs = ["First sentence.| Second sentence.| Third."]
        agent = VoiceAgent("voice", config, client)  # type: ignore[arg-type]

        chunks = [part async for part in agent.stream("Say something.")]
        self.assertEqual(chunks, ["First sentence. ", "Second sentence. ", "Third. "])

    async def test_automation_agent_retries_once_then_returns_validated_object(self) -> None:
        config = load_config()
        client = FakeClient()
        client.outputs = [
            '{"action":"open_app"}',
            '{"action":"open_app","arguments":{"appName":"Calculator"}}',
        ]
        agent = AutomationAgent("automation", config, client)  # type: ignore[arg-type]
        schema = {
            "type": "object",
            "properties": {"action": {"type": "string"}, "arguments": {"type": "object"}},
            "required": ["action", "arguments"],
        }

        result = await agent.complete("Open Calculator", schema)
        self.assertEqual(result["arguments"]["appName"], "Calculator")
        self.assertEqual(len(client.calls), 2)
        self.assertEqual(client.calls[0]["response_format"], schema)
        self.assertTrue(client.calls[1]["messages"][-1]["content"].startswith(
            "The previous response was invalid."
        ))

    async def test_automation_agent_surfaces_second_invalid_response(self) -> None:
        client = FakeClient()
        client.outputs = ["invalid one", "invalid two"]
        agent = AutomationAgent("automation", load_config(), client)  # type: ignore[arg-type]
        with self.assertRaisesRegex(OllamaError, "after retry"):
            await agent.complete("Do something", {"type": "object"})

    async def test_complex_route_falls_back_without_swapping_unloaded_heavy_model(self) -> None:
        config = load_config()
        client = FakeClient()
        events = [
            event async for event in Orchestrator(config, client).stream(
                "complex task", task="complex"
            )
        ]
        self.assertEqual(events[0].data["agent"], "chat")
        self.assertTrue(events[0].data["fallback"])
        self.assertTrue(any(call for call in client.calls if call["model"] == config["profiles"]["fast"]["models"]["chat"]))
        self.assertFalse(any(call["model"] == config["profiles"]["heavy"]["model"] for call in client.calls))


if __name__ == "__main__":
    unittest.main()
