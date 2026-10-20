"""Structured planner that runs only on an already-loaded small router model."""

from __future__ import annotations

import json
from typing import Any

from jsonschema import ValidationError, validate

from .client import OllamaClient, OllamaError

PLAN_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "subtasks": {
            "type": "array",
            "items": {"type": "string", "minLength": 1},
            "minItems": 2,
            "maxItems": 8,
        }
    },
    "required": ["subtasks"],
    "additionalProperties": False,
}


class Planner:
    def __init__(self, config: dict[str, Any], client: OllamaClient) -> None:
        self.config = config
        self.client = client

    async def plan(self, prompt: str) -> list[str] | None:
        planner_config = self.config.get("planner", {})
        if not planner_config.get("enabled", True):
            return None

        model_name = str(self.config["profiles"]["fast"]["models"]["router"])
        loaded = {item.get("name") for item in await self.client.loaded_models()}
        if str(self.config["profiles"]["heavy"]["model"]) in loaded:
            return None
        if model_name not in loaded:
            return None

        max_subtasks = min(8, max(2, int(planner_config.get("max_subtasks", 4))))
        schema = {
            **PLAN_SCHEMA,
            "properties": {
                "subtasks": {
                    **PLAN_SCHEMA["properties"]["subtasks"],
                    "maxItems": max_subtasks,
                }
            },
        }
        fast = self.config["profiles"]["fast"]
        result = await self.client.stream_chat(
            model=model_name,
            messages=[
                {
                    "role": "system",
                    "content": (
                        "Break this request into independent, self-contained subtasks. "
                        "Return only the JSON object required by the schema."
                    ),
                },
                {"role": "user", "content": prompt},
            ],
            options={"temperature": 0, "num_predict": 256, "num_ctx": 2048},
            keep_alive=(
                fast.get("keep_alive")
                if self.config.get("optimizations", {}).get("keep_alive", True)
                else None
            ),
            response_format=schema,
        )
        try:
            value = json.loads(result.text)
            validate(instance=value, schema=schema)
        except (json.JSONDecodeError, ValidationError) as error:
            raise OllamaError(f"Planner returned an invalid subtask plan: {error}") from error

        subtasks = [task.strip() for task in value["subtasks"]]
        if any(not task for task in subtasks):
            raise OllamaError("Planner returned a blank subtask.")
        if len({task.casefold() for task in subtasks}) != len(subtasks):
            raise OllamaError("Planner returned duplicate subtasks.")
        return subtasks
