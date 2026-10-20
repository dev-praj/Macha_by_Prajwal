"""Route and stream a request through its configured local agent."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any

from jsonschema import Draft202012Validator, SchemaError, ValidationError, validate

from .agents import AutomationAgent, create_agents
from .cache import ResponseCache
from .client import OllamaClient, OllamaError
from .memory import ConversationMemory
from .planner import Planner
from .router import TaskRoute, TaskRouter

logger = logging.getLogger(__name__)


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
        self.planner = Planner(config, client)
        self.memory = ConversationMemory(config, client, self.agents["summarize"])
        self.cache = ResponseCache(config.get("response_cache", {}))

    async def stream(
        self,
        prompt: str,
        *,
        task: str | None = None,
        history: list[dict[str, Any]] | None = None,
        tool_schema: dict[str, Any] | None = None,
        conversation_id: str | None = None,
        generation_options: dict[str, Any] | None = None,
    ) -> AsyncIterator[StreamEvent]:
        if not prompt.strip() and not history:
            raise ValueError("A non-empty prompt or conversation history is required.")
        started = time.perf_counter()
        history = history or []
        route = await self.router.route(prompt, task)
        history = await self.memory.prepare(
            prompt,
            history,
            conversation_id,
            str(self.config["agents"][route.task]["system"]),
        )
        effective_route = route
        agent = self.agents[route.task]
        plan: list[str] | None = None
        subtask_results: list[str] = []
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
        if route.task == "complex" and effective_route.task == "chat":
            try:
                plan = await self.planner.plan(prompt)
            except OllamaError as error:
                logger.warning("Complex-task planning failed; using a single fast response: %s", error)
            if plan:
                concurrency = max(
                    1, int(self.config.get("planner", {}).get("max_concurrency", 2))
                )
                semaphore = asyncio.Semaphore(concurrency)
                chat_agent = self.agents["chat"]

                async def execute(subtask: str) -> str:
                    async with semaphore:
                        chunks = [
                            chunk async for chunk in chat_agent.stream(subtask, history)
                        ]
                    return "".join(chunks)

                subtask_results = await asyncio.gather(
                    *(execute(subtask) for subtask in plan)
                )

        yield StreamEvent(
            "meta",
            {
                "task": route.task,
                "agent": effective_route.task,
                "route_source": route.source,
                "fallback": effective_route.source.startswith("fast-fallback"),
                "planned_subtasks": len(plan or []),
            },
        )
        if isinstance(agent, AutomationAgent):
            schema = tool_schema if tool_schema is not None else {
                "type": "object",
                "properties": {"result": {"type": "string"}},
                "required": ["result"],
                "additionalProperties": False,
            }
            if schema.get("type") != "object":
                raise ValueError("Automation tool schema must describe a top-level JSON object.")
            try:
                Draft202012Validator.check_schema(schema)
            except SchemaError as error:
                raise ValueError(f"Automation tool schema is invalid: {error.message}") from error
            cache_key = self._cache_key(
                "automation",
                prompt,
                history,
                {"schema": schema, "generation_options": generation_options},
            )
            value = await self.cache.get(cache_key) if cache_key else None
            if value is None:
                value = await agent.complete(prompt, schema, history, generation_options)
                if cache_key:
                    await self.cache.put(cache_key, value)
            else:
                try:
                    validate(instance=value, schema=schema)
                except ValidationError as error:
                    raise OllamaError(
                        f"Cached automation result does not match the requested schema: {error}"
                    ) from error
            content = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
            ttft = time.perf_counter() - started
            yield StreamEvent("token", {"content": content, "ttft_seconds": ttft})
        else:
            first = True
            selected_prompt = prompt
            if route.task == "complex" and effective_route.task == "chat":
                if subtask_results:
                    selected_prompt = (
                        "Answer the original request using these independent subtask results. "
                        "Reconcile them and clearly identify any uncertainty.\n\n"
                        f"Original request:\n{prompt}\n\n"
                        + "\n\n".join(
                            f"Subtask {index} result:\n{result}"
                            for index, result in enumerate(subtask_results, start=1)
                        )
                    )
                else:
                    selected_prompt = (
                        "The complex model is not currently loaded; answer this task using the fast profile. "
                        + prompt
                    )
            cache_key = (
                self._cache_key(
                    "summarize",
                    selected_prompt,
                    history,
                    generation_options,
                )
                if route.task == "summarize"
                else None
            )
            cached = await self.cache.get(cache_key) if cache_key else None
            if isinstance(cached, str):
                first = False
                yield StreamEvent(
                    "token",
                    {"content": cached, "ttft_seconds": time.perf_counter() - started},
                )
            elif cached is not None:
                raise OllamaError("Cached summary result is not a string.")
            else:
                generated: list[str] = []
                async for chunk in agent.stream(
                    selected_prompt,
                    history,
                    generation_options,
                ):
                    generated.append(chunk)
                    event_data: dict[str, Any] = {"content": chunk}
                    if first:
                        event_data["ttft_seconds"] = time.perf_counter() - started
                        first = False
                    yield StreamEvent("token", event_data)
                if cache_key:
                    await self.cache.put(cache_key, "".join(generated))
        yield StreamEvent(
            "done",
            {"task": route.task, "agent": effective_route.task},
        )

    def _cache_key(
        self,
        agent_name: str,
        prompt: str,
        history: list[dict[str, Any]],
        extra: Any = None,
    ) -> str | None:
        if (
            not self.cache.enabled
            or agent_name not in self.cache.deterministic_agents
        ):
            return None
        agent = self.agents[agent_name]
        return self.cache.make_key(
            agent_name,
            prompt,
            history,
            {
                "request": extra,
                "model": agent.model_name(),
                "system": self.config["agents"][agent_name]["system"],
            },
        )
