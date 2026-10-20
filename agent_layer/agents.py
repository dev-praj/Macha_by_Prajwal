"""Small, config-driven Ollama agent implementations."""

from __future__ import annotations

import json
import re
from collections.abc import AsyncIterator
from typing import Any

from jsonschema import ValidationError, validate

from .client import OllamaClient, OllamaError


class Agent:
    def __init__(
        self,
        name: str,
        config: dict[str, Any],
        client: OllamaClient,
    ) -> None:
        self.name = name
        self.config = config
        self.client = client

    def model_name(self) -> str:
        profile = self.config["profiles"][self.config["agents"][self.name]["profile"]]
        model_key = self.config["agents"][self.name]["model_key"]
        return str(profile["models"][model_key] if "models" in profile else profile[model_key])

    def options(self) -> dict[str, Any]:
        agent = self.config["agents"][self.name]
        return {
            "temperature": float(agent["temperature"]),
            "num_predict": int(agent["num_predict"]),
            "num_ctx": int(agent["num_ctx"]),
        }

    def messages(
        self,
        prompt: str,
        history: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        system = str(self.config["agents"][self.name]["system"])
        # Keep the static system content byte-for-byte stable. Dynamic user
        # content and prior turns always follow it.
        return [{"role": "system", "content": system}, *history, {"role": "user", "content": prompt}]

    async def stream(
        self,
        prompt: str,
        history: list[dict[str, Any]] | None = None,
    ) -> AsyncIterator[str]:
        async for chunk in self.client.chat_stream(
            model=self.model_name(),
            messages=self.messages(prompt, history or []),
            options=self.options(),
        ):
            yield chunk


class VoiceAgent(Agent):
    """Stream complete short sentences as soon as sentence punctuation arrives."""

    async def stream(
        self,
        prompt: str,
        history: list[dict[str, Any]] | None = None,
    ) -> AsyncIterator[str]:
        buffer = ""
        async for chunk in self.client.chat_stream(
            model=self.model_name(),
            messages=self.messages(prompt, history or []),
            options=self.options(),
        ):
            buffer += chunk
            while match := re.search(r".*?[.!?](?:\s+|$)", buffer, re.S):
                sentence = match.group(0)
                buffer = buffer[len(sentence):]
                yield sentence.strip() + " "
        if buffer.strip():
            yield buffer.strip()


class AutomationAgent(Agent):
    async def complete(
        self,
        prompt: str,
        schema: dict[str, Any],
        history: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        messages = self.messages(prompt, history or [])
        messages[0] = {
            "role": "system",
            "content": f"{messages[0]['content']}\n\nReturn a JSON value matching this schema.",
        }
        user_message = messages[-1]
        user_message["content"] = (
            f"{user_message['content']}\n\n"
            f"JSON schema:\n{json.dumps(schema, separators=(',', ':'))}"
        )
        last_error = "The model returned invalid JSON."
        for attempt in range(2):
            result = await self.client.stream_chat(
                model=self.model_name(),
                messages=messages,
                options=self.options(),
                response_format=schema,
            )
            try:
                value = json.loads(result.text)
                if not isinstance(value, dict):
                    raise ValueError("Expected a JSON object.")
                validate(instance=value, schema=schema)
                return value
            except (json.JSONDecodeError, ValueError, ValidationError) as error:
                last_error = str(error)
                if attempt == 0:
                    messages.append({"role": "assistant", "content": result.text})
                    messages.append({
                        "role": "user",
                        "content": "The previous response was invalid. Return a valid JSON object matching the required schema only.",
                    })
        raise OllamaError(f"Automation agent could not produce valid JSON after retry: {last_error}")


class HeavyAgent(Agent):
    """Use the heavy model only when already loaded; never trigger an implicit swap."""

    async def stream(
        self,
        prompt: str,
        history: list[dict[str, Any]] | None = None,
    ) -> AsyncIterator[str]:
        loaded = await self.client.loaded_models()
        if not any(item.get("name") == self.model_name() for item in loaded):
            raise OllamaError(
                "Heavy model is not already loaded; Phase 2 will not swap models. "
                "Retry with the fast profile or authorize heavy-model loading."
            )
        async for chunk in super().stream(prompt, history):
            yield chunk


def create_agents(
    config: dict[str, Any],
    client: OllamaClient,
) -> dict[str, Agent]:
    return {
        "chat": Agent("chat", config, client),
        "voice": VoiceAgent("voice", config, client),
        "automation": AutomationAgent("automation", config, client),
        "code": Agent("code", config, client),
        "summarize": Agent("summarize", config, client),
        "complex": HeavyAgent("complex", config, client),
    }
