"""Fast rule-based task routing with a guarded Ollama JSON fallback."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any

from .client import OllamaClient, OllamaError

TASKS = {"chat", "voice", "automation", "code", "summarize", "complex"}
ROUTE_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "task": {"type": "string", "enum": sorted(TASKS)},
    },
    "required": ["task"],
    "additionalProperties": False,
}

RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("voice", re.compile(r"\b(say aloud|spoken|voice reply|keep it short|quick answer)\b", re.I)),
    ("automation", re.compile(r"\b(open|launch|click|search the web|create a file|remember this|run this)\b", re.I)),
    ("code", re.compile(r"\b(code|program|function|debug|compile|typescript|python|rust|javascript|sql)\b", re.I)),
    ("summarize", re.compile(r"\b(summarize|summary|tl;dr|shorten|condense)\b", re.I)),
    ("complex", re.compile(r"\b(complex|comprehensive|multi.?step|compare and design|architecture|trade.?off analysis)\b", re.I)),
)


@dataclass(frozen=True)
class TaskRoute:
    task: str
    source: str


class TaskRouter:
    def __init__(self, config: dict[str, Any], client: OllamaClient) -> None:
        self.config = config
        self.client = client

    async def route(self, text: str, explicit_task: str | None = None) -> TaskRoute:
        if explicit_task is not None:
            if explicit_task not in TASKS:
                raise ValueError(f"Unsupported task type: {explicit_task}")
            return TaskRoute(explicit_task, "request")

        for task, pattern in RULES:
            if pattern.search(text):
                return TaskRoute(task, "rules")

        # The router only uses a model that is already resident; routing never
        # implicitly loads a model or changes model tiers.
        try:
            router_model = self._router_model()
            loaded = {item.get("name") for item in await self.client.loaded_models()}
            if router_model not in loaded:
                return TaskRoute("chat", "rules-fallback-router-model-not-loaded")
            result = await self.client.stream_chat(
                model=router_model,
                messages=[
                    {"role": "system", "content": "Classify the request. Return only the task label allowed by the JSON schema."},
                    {"role": "user", "content": text},
                ],
                options={"temperature": 0, "num_predict": 32, "num_ctx": 1024},
                keep_alive=(
                    self.config["profiles"]["fast"].get("keep_alive")
                    if self.config.get("optimizations", {}).get("keep_alive", True)
                    else None
                ),
                response_format=ROUTE_SCHEMA,
            )
            parsed = json.loads(result.text)
            task = parsed.get("task") if isinstance(parsed, dict) else None
            if task in TASKS:
                return TaskRoute(task, "model")
        except (KeyError, TypeError, ValueError, OllamaError, json.JSONDecodeError):
            pass
        return TaskRoute("chat", "rules-fallback")

    def _router_model(self) -> str:
        return str(self.config["profiles"]["fast"]["models"]["router"])
