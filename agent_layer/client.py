"""Async streaming client for Ollama's native chat API."""

from __future__ import annotations

import json
import time
from dataclasses import dataclass
from typing import Any, AsyncIterator

import httpx


class OllamaError(RuntimeError):
    """An Ollama request failed or returned an invalid stream."""


@dataclass(frozen=True)
class StreamResult:
    text: str
    time_to_first_token_seconds: float | None
    total_latency_seconds: float
    prompt_tokens: int
    generated_tokens: int
    load_duration_seconds: float
    eval_duration_seconds: float
    final_message: dict[str, Any]

    @property
    def tokens_per_second(self) -> float | None:
        if self.eval_duration_seconds <= 0:
            return None
        return self.generated_tokens / self.eval_duration_seconds


class OllamaClient:
    def __init__(
        self,
        base_url: str,
        timeout_seconds: float = 180,
        client: httpx.AsyncClient | None = None,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = httpx.Timeout(timeout_seconds)
        self._client = client
        self._owns_client = client is None

    async def __aenter__(self) -> OllamaClient:
        if self._client is None:
            self._client = httpx.AsyncClient(timeout=self.timeout)
        return self

    async def __aexit__(self, *_: object) -> None:
        if self._owns_client and self._client is not None:
            await self._client.aclose()
            self._client = None

    async def list_models(self) -> list[dict[str, Any]]:
        response = await self._request("GET", "/api/tags")
        models = response.get("models")
        if not isinstance(models, list):
            raise OllamaError("Ollama model list response is invalid.")
        return models

    async def loaded_models(self) -> list[dict[str, Any]]:
        response = await self._request("GET", "/api/ps")
        models = response.get("models")
        if not isinstance(models, list):
            raise OllamaError("Ollama loaded-model response is invalid.")
        return models

    async def stream_chat(
        self,
        *,
        model: str,
        messages: list[dict[str, Any]],
        options: dict[str, Any] | None = None,
        keep_alive: str | int | None = None,
        response_format: dict[str, Any] | str | None = None,
    ) -> StreamResult:
        client = self._require_client()
        payload: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "stream": True,
        }
        if options is not None:
            payload["options"] = options
        if keep_alive is not None:
            payload["keep_alive"] = keep_alive
        if response_format is not None:
            payload["format"] = response_format

        start = time.perf_counter()
        first_token_at: float | None = None
        chunks: list[str] = []
        final_message: dict[str, Any] = {}
        stats: dict[str, Any] = {}
        try:
            async with client.stream(
                "POST", f"{self.base_url}/api/chat", json=payload
            ) as response:
                await self._raise_for_status(response)
                async for line in response.aiter_lines():
                    if not line:
                        continue
                    try:
                        event = json.loads(line)
                    except json.JSONDecodeError as error:
                        raise OllamaError("Ollama returned invalid JSON in its stream.") from error
                    if not isinstance(event, dict):
                        raise OllamaError("Ollama returned an invalid stream event.")
                    message = event.get("message")
                    if isinstance(message, dict):
                        final_message = message
                        content = message.get("content")
                        if isinstance(content, str) and content:
                            if first_token_at is None:
                                first_token_at = time.perf_counter()
                            chunks.append(content)
                    stats.update(event)
                    if event.get("error"):
                        raise OllamaError(str(event["error"]))
                    if event.get("done") is True:
                        break
        except httpx.TimeoutException as error:
            raise OllamaError(f"Ollama request timed out: {error}") from error
        except httpx.HTTPError as error:
            raise OllamaError(f"Could not reach Ollama: {error}") from error

        total = time.perf_counter() - start
        if not stats.get("done", False):
            raise OllamaError("Ollama stream ended before a final response was received.")
        return StreamResult(
            text="".join(chunks),
            time_to_first_token_seconds=(
                first_token_at - start if first_token_at is not None else None
            ),
            total_latency_seconds=total,
            prompt_tokens=int(stats.get("prompt_eval_count", 0)),
            generated_tokens=int(stats.get("eval_count", 0)),
            load_duration_seconds=int(stats.get("load_duration", 0)) / 1_000_000_000,
            eval_duration_seconds=int(stats.get("eval_duration", 0)) / 1_000_000_000,
            final_message=final_message,
        )

    async def chat_stream(
        self,
        *,
        model: str,
        messages: list[dict[str, Any]],
        options: dict[str, Any] | None = None,
        keep_alive: str | int | None = None,
        response_format: dict[str, Any] | str | None = None,
    ) -> AsyncIterator[str]:
        client = self._require_client()
        payload: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "stream": True,
        }
        if options is not None:
            payload["options"] = options
        if keep_alive is not None:
            payload["keep_alive"] = keep_alive
        if response_format is not None:
            payload["format"] = response_format
        try:
            async with client.stream(
                "POST", f"{self.base_url}/api/chat", json=payload
            ) as response:
                await self._raise_for_status(response)
                async for line in response.aiter_lines():
                    if not line:
                        continue
                    try:
                        event = json.loads(line)
                    except json.JSONDecodeError as error:
                        raise OllamaError("Ollama returned invalid JSON in its stream.") from error
                    if not isinstance(event, dict):
                        raise OllamaError("Ollama returned an invalid stream event.")
                    if event.get("error"):
                        raise OllamaError(str(event["error"]))
                    message = event.get("message")
                    if isinstance(message, dict):
                        content = message.get("content")
                        if isinstance(content, str) and content:
                            yield content
                    if event.get("done") is True:
                        return
            raise OllamaError("Ollama stream ended before a final response was received.")
        except httpx.TimeoutException as error:
            raise OllamaError(f"Ollama request timed out: {error}") from error
        except httpx.HTTPError as error:
            raise OllamaError(f"Could not reach Ollama: {error}") from error

    async def _request(self, method: str, path: str) -> dict[str, Any]:
        try:
            response = await self._require_client().request(
                method, f"{self.base_url}{path}"
            )
            response.raise_for_status()
            data = response.json()
        except httpx.TimeoutException as error:
            raise OllamaError(f"Ollama request timed out: {error}") from error
        except httpx.HTTPError as error:
            raise OllamaError(f"Could not reach Ollama: {error}") from error
        except json.JSONDecodeError as error:
            raise OllamaError("Ollama returned invalid JSON.") from error
        if not isinstance(data, dict):
            raise OllamaError("Ollama returned an invalid response.")
        return data

    async def _raise_for_status(self, response: httpx.Response) -> None:
        if response.is_success:
            return
        body = (await response.aread()).decode("utf-8", errors="replace")
        raise OllamaError(f"Ollama returned HTTP {response.status_code}: {body[:300]}")

    def _require_client(self) -> httpx.AsyncClient:
        if self._client is None:
            raise OllamaError("Use OllamaClient as an async context manager.")
        return self._client
