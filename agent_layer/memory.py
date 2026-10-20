"""Opt-in bounded conversation history with rolling summaries."""

from __future__ import annotations

import hashlib
import json
from collections import OrderedDict
from typing import Any

from .agents import Agent
from .client import OllamaClient, OllamaError


def estimate_tokens(text: str) -> int:
    """Conservative rough estimate used when no tokenizer is available."""
    return max(1, (len(text) + 2) // 3)


def _history_digest(history: list[dict[str, Any]]) -> str:
    encoded = json.dumps(history, sort_keys=True, ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


class ConversationMemory:
    def __init__(
        self,
        config: dict[str, Any],
        client: OllamaClient,
        summarizer: Agent,
    ) -> None:
        self.config = config
        self.client = client
        self.summarizer = summarizer
        self._summaries: OrderedDict[str, tuple[str, int, str]] = OrderedDict()

    async def prepare(
        self,
        prompt: str,
        history: list[dict[str, Any]],
        conversation_id: str | None = None,
        system_prompt: str = "",
    ) -> list[dict[str, Any]]:
        settings = self.config.get("memory", {})
        if not settings.get("enabled", False):
            return history

        for message in history:
            if (
                message.get("role") not in {"system", "user", "assistant"}
                or not isinstance(message.get("content"), str)
            ):
                raise ValueError("Conversation history must contain supported roles and string content.")

        token_budget = max(1, int(settings.get("token_budget", 3072)))
        if self._token_count(prompt, history, system_prompt) <= token_budget:
            return history

        last_n_turns = max(1, int(settings.get("last_n_turns", 8)))
        user_turns = [index for index, message in enumerate(history) if message["role"] == "user"]
        if len(user_turns) <= last_n_turns:
            raise OllamaError(
                "Conversation exceeds its token budget, but there are no older complete turns "
                "to summarize. Shorten the prompt or increase memory.token_budget."
            )
        cutoff = user_turns[-last_n_turns]
        older, recent = history[:cutoff], history[cutoff:]
        old_messages = [message for message in older if message["role"] != "system"]
        pinned_system = [message for message in older if message["role"] == "system"]
        if not old_messages:
            raise OllamaError("Conversation exceeds its token budget and has no summarizable turns.")

        summary = await self._summarize(old_messages, conversation_id)
        summary_message = {
            "role": "system",
            "content": f"Summary of older conversation turns:\n{summary}",
        }
        bounded = [*pinned_system, summary_message, *recent]
        if self._token_count(prompt, bounded, system_prompt) > token_budget:
            raise OllamaError(
                "Conversation remains over its token budget after summarization. "
                "Increase memory.token_budget or shorten the prompt."
            )
        return bounded

    async def _summarize(
        self,
        messages: list[dict[str, Any]],
        conversation_id: str | None,
    ) -> str:
        previous: tuple[str, int, str] | None = None
        if conversation_id:
            previous = self._summaries.get(conversation_id)
            if previous is not None:
                self._summaries.move_to_end(conversation_id)
        if previous and previous[1] <= len(messages):
            prior_count = previous[1]
            prior_digest = _history_digest(messages[:prior_count])
            if prior_digest == previous[2] and prior_count == len(messages):
                return previous[0]
            if prior_digest == previous[2]:
                additions = messages[prior_count:]
                if not additions:
                    return previous[0]
                content = (
                    f"Update this rolling conversation summary with the newly older turns.\n"
                    f"Existing summary:\n{previous[0]}\n"
                    f"New turns:\n{self._format_messages(additions)}"
                )
            else:
                previous = None
                content = self._format_messages(messages)
        else:
            content = self._format_messages(messages)

        model_name = self.summarizer.model_name()
        loaded = {item.get("name") for item in await self.client.loaded_models()}
        if str(self.config["profiles"]["heavy"]["model"]) in loaded:
            raise OllamaError(
                "Conversation compression is unavailable while the heavy model is loaded; "
                "history was not discarded."
            )
        if model_name not in loaded:
            raise OllamaError(
                f"Conversation compression requires already-loaded summarizer model "
                f"{model_name!r}; no model will be loaded automatically."
            )
        result = await self.client.stream_chat(
            model=model_name,
            messages=[
                {
                    "role": "system",
                    "content": str(self.config["agents"]["summarize"]["system"]),
                },
                {
                    "role": "user",
                    "content": (
                        "Preserve important facts, decisions, and unresolved questions. "
                        "Do not invent details.\n\n" + content
                    ),
                },
            ],
            options=self.summarizer.options(),
            keep_alive=self.summarizer.keep_alive(),
        )
        if not result.text.strip():
            raise OllamaError("Conversation summarizer returned an empty summary.")
        if conversation_id:
            self._summaries[conversation_id] = (
                result.text,
                len(messages),
                _history_digest(messages),
            )
            self._summaries.move_to_end(conversation_id)
            max_conversations = max(
                1,
                int(self.config.get("memory", {}).get("max_conversations", 128)),
            )
            while len(self._summaries) > max_conversations:
                self._summaries.popitem(last=False)
        return result.text

    @staticmethod
    def _format_messages(messages: list[dict[str, Any]]) -> str:
        return "\n".join(
            f"{message['role']}: {message['content']}" for message in messages
        )

    @staticmethod
    def _token_count(
        prompt: str,
        history: list[dict[str, Any]],
        system_prompt: str = "",
    ) -> int:
        text = system_prompt + "\n" + prompt + "\n" + "\n".join(
            str(message.get("content", "")) for message in history
        )
        return estimate_tokens(text)
