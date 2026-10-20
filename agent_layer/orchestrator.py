"""Route and stream a request through its configured local agent."""

from __future__ import annotations

import json
import time
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any

from .agents import AutomationAgent, create_agents
from .client import OllamaClient, OllamaError
from .router import TaskRoute, TaskRouter


@dataclass(frozen=True)
class StreamEvent:
    kind: str
    data: dict[str, Any]


class Orchestrator:
    def __init__(self, config: dict[str, Any], client: OllamaClient) -> None:
        self.config = config
        self.client = client
        self.router = TaskRouter(config, client)
        self.agents = create_agents(config, client)

    async def stream(
        self,
        prompt: str,
        *,
        task: str | None = None,
        history: list[dict[str, Any]] | None = None,
        tool_schema: dict[str, Any] | None = None,
    ) -> AsyncIterator[StreamEvent]:
        if not prompt.strip() and not history:
            raise ValueError("A non-empty prompt or conversation history is required.")
        history = history or []
        route = await self.router.route(prompt, task)
        effective_route = route
        agent = self.agents[route.task]
        if route.task == "complex":
            try:
                loaded_names = {
                    str(item.get("name"))
                    for item in await self.client.loaded_models()
                }
                if agent.model_name() not in loaded_names:
                    effective_route = TaskRoute("chat", "fast-fallback-heavy-model-not-loaded")
                    agent = self.agents["chat"]
            except OllamaError:
                effective_route = TaskRoute("chat", "fast-fallback-ollama-status-unavailable")
                agent = self.agents["chat"]

        started = time.perf_counter()
        yield StreamEvent(
            "meta",
            {
                "task": route.task,
                "agent": effective_route.task,
                "route_source": route.source,
                "fallback": effective_route.source.startswith("fast-fallback"),
            },
        )
        if isinstance(agent, AutomationAgent):
            schema = tool_schema or {
                "type": "object",
                "properties": {"result": {"type": "string"}},
                "required": ["result"],
                "additionalProperties": False,
            }
            value = await agent.complete(prompt, schema, history)
            content = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
            ttft = time.perf_counter() - started
            yield StreamEvent("token", {"content": content, "ttft_seconds": ttft})
        else:
            first = True
            selected_prompt = prompt
            if route.task == "complex" and effective_route.task == "chat":
                selected_prompt = (
                    "The complex model is not currently loaded; answer this task using the fast profile. "
                    + prompt
                )
            async for chunk in agent.stream(selected_prompt, history):
                event_data: dict[str, Any] = {"content": chunk}
                if first:
                    event_data["ttft_seconds"] = time.perf_counter() - started
                    first = False
                yield StreamEvent("token", event_data)
        yield StreamEvent(
            "done",
            {"task": route.task, "agent": effective_route.task},
        )
