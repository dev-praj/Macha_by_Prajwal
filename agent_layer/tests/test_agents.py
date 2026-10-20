from __future__ import annotations

import asyncio
import unittest
from collections.abc import AsyncIterator
from typing import Any

from agent_layer.agents import AutomationAgent, VoiceAgent, create_agents
from agent_layer.client import OllamaError, StreamResult
from agent_layer.api import load_config
from agent_layer.orchestrator import Orchestrator
from agent_layer.planner import Planner
from agent_layer.router import TaskRouter


class FakeClient:
    def __init__(self) -> None:
        self.calls: list[dict[str, Any]] = []
        self.outputs: list[str] = []
        self.installed: list[dict[str, Any]] = []
        self.loaded: list[dict[str, Any]] = []
        self.active_streams = 0
        self.max_active_streams = 0

    async def chat_stream(self, **kwargs: Any) -> AsyncIterator[str]:
        self.calls.append(kwargs)
        output = self.outputs.pop(0) if self.outputs else "ok"
        self.active_streams += 1
        self.max_active_streams = max(self.max_active_streams, self.active_streams)
        try:
            await asyncio.sleep(0.001)
            for part in output.split("|"):
                yield part
        finally:
            self.active_streams -= 1

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

    async def test_router_uses_json_model_only_when_ambiguous_and_loaded(self) -> None:
        config = load_config()
        model = config["profiles"]["fast"]["models"]["router"]
        client = FakeClient()
        client.installed = [{"name": model}]
        client.loaded = [{"name": model}]
        client.outputs = ['{"task":"code"}']
        route = await TaskRouter(config, client).route("Could this be useful?")

        self.assertEqual((route.task, route.source), ("code", "model"))
        self.assertEqual(client.calls[0]["response_format"]["type"], "object")

    async def test_router_falls_back_without_pulling_missing_router_model(self) -> None:
        client = FakeClient()
        route = await TaskRouter(load_config(), client).route("Can you help me?")
        self.assertEqual(route.task, "chat")
        self.assertEqual(route.source, "rules-fallback-router-model-not-loaded")

    async def test_planner_only_uses_an_already_loaded_router_model(self) -> None:
        config = load_config()
        client = FakeClient()
        self.assertIsNone(await Planner(config, client).plan("Compare options"))
        self.assertEqual(client.calls, [])

        model = config["profiles"]["fast"]["models"]["router"]
        client.loaded = [{"name": model}]
        client.outputs = ['{"subtasks":["Compare costs","Compare reliability"]}']
        self.assertEqual(
            await Planner(config, client).plan("Compare options"),
            ["Compare costs", "Compare reliability"],
        )
        self.assertEqual(client.calls[0]["response_format"]["properties"]["subtasks"]["maxItems"], 4)

    async def test_planner_skips_fast_model_when_heavy_model_is_resident(self) -> None:
        config = load_config()
        client = FakeClient()
        client.loaded = [
            {"name": config["profiles"]["fast"]["models"]["router"]},
            {"name": config["profiles"]["heavy"]["model"]},
        ]

        self.assertIsNone(await Planner(config, client).plan("Compare options"))
        self.assertEqual(client.calls, [])

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
        self.assertEqual(agent.keep_alive(), -1)
        self.assertEqual(agents["complex"].keep_alive(), "2m")
        self.assertEqual(agents["complex"].think_level(), "medium")
        self.assertEqual(agent.options({"num_predict": 1000})["num_predict"], 256)

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

    async def test_complex_fast_fallback_runs_planned_subtasks_concurrently(self) -> None:
        config = load_config()
        client = FakeClient()
        client.loaded = [{"name": config["profiles"]["fast"]["models"]["router"]}]
        client.outputs = ['{"subtasks":["Compare costs","Compare reliability"]}']
        events = [
            event async for event in Orchestrator(config, client).stream(
                "Compare both options", task="complex"
            )
        ]

        self.assertEqual(events[0].data["planned_subtasks"], 2)
        self.assertEqual(client.max_active_streams, 2)
        self.assertEqual(events[-1].kind, "done")
        self.assertFalse(any(
            call.get("model") == config["profiles"]["heavy"]["model"]
            for call in client.calls
        ))

    async def test_deterministic_automation_results_are_cached(self) -> None:
        config = load_config()
        config["response_cache"]["enabled"] = True
        client = FakeClient()
        client.outputs = ['{"result":"done"}']
        orchestrator = Orchestrator(config, client)  # type: ignore[arg-type]

        first = [
            event async for event in orchestrator.stream("Do a task", task="automation")
        ]
        second = [
            event async for event in orchestrator.stream("  DO   A TASK  ", task="automation")
        ]

        self.assertEqual(len(client.calls), 1)
        self.assertEqual(first[-2].data["content"], second[-2].data["content"])

    async def test_summarize_results_are_cached(self) -> None:
        config = load_config()
        config["response_cache"]["enabled"] = True
        client = FakeClient()
        orchestrator = Orchestrator(config, client)  # type: ignore[arg-type]

        first = [event async for event in orchestrator.stream("short summary", task="summarize")]
        second = [event async for event in orchestrator.stream("SHORT SUMMARY", task="summarize")]
        self.assertEqual(len(client.calls), 1)
        self.assertEqual(first[-2].data["content"], second[-2].data["content"])

    async def test_chat_and_voice_are_not_cached_by_default(self) -> None:
        config = load_config()
        config["response_cache"]["enabled"] = True
        client = FakeClient()
        orchestrator = Orchestrator(config, client)  # type: ignore[arg-type]

        for task in ("chat", "voice"):
            for _ in range(2):
                _ = [
                    event async for event in orchestrator.stream(
                        "same request", task=task
                    )
                ]

        self.assertEqual(len(client.calls), 4)


if __name__ == "__main__":
    unittest.main()
