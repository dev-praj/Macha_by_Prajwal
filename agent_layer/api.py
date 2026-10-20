"""FastAPI streaming API for local Ollama agents."""

from __future__ import annotations

import json
import time
import uuid
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, AsyncGenerator, AsyncIterator

import yaml
from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse, Response, StreamingResponse
from jsonschema import Draft202012Validator, SchemaError
from pydantic import BaseModel, Field

from .client import OllamaClient, OllamaError
from .orchestrator import Orchestrator
from .runtime import initialize_runtime

ROOT = Path(__file__).resolve().parent


class ChatRequest(BaseModel):
    prompt: str = ""
    task: str | None = None
    history: list[dict[str, Any]] = Field(default_factory=list)
    tool_schema: dict[str, Any] | None = None
    conversation_id: str | None = None


class OpenAIChatRequest(BaseModel):
    model: str | None = None
    messages: list[dict[str, Any]]
    stream: bool = True
    temperature: float | None = Field(default=None, ge=0, le=2)
    max_tokens: int | None = Field(default=None, ge=1)
    response_format: dict[str, Any] | None = None


def load_config(config_path: Path = ROOT / "config.yaml") -> dict[str, Any]:
    try:
        config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as error:
        raise RuntimeError(f"Could not load agent-layer config: {error}") from error
    if not isinstance(config, dict):
        raise RuntimeError("Agent-layer config must be a YAML mapping.")
    return config


def sse(event: str, data: dict[str, Any]) -> str:
    return f"event: {event}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n"


def openai_messages(
    messages: list[dict[str, Any]],
) -> tuple[str, list[dict[str, Any]]]:
    if not messages or messages[-1].get("role") != "user":
        raise ValueError("The final OpenAI message must be from the user.")
    prompt = messages[-1].get("content", "")
    if not isinstance(prompt, str) or not prompt:
        raise ValueError("The final user message must have non-empty string content.")
    history: list[dict[str, Any]] = []
    for message in messages[:-1]:
        role = message.get("role")
        content = message.get("content", "")
        if role not in {"system", "user", "assistant"}:
            raise ValueError(f"Unsupported OpenAI message role: {role!r}")
        if not isinstance(content, str):
            raise ValueError("This endpoint currently requires string message content.")
        history.append({"role": role, "content": content})
    return prompt, history


def response_format_options(
    response_format: dict[str, Any] | None,
) -> tuple[str | None, dict[str, Any] | None]:
    if response_format is None or response_format.get("type") == "text":
        return None, None
    format_type = response_format.get("type")
    if format_type == "json_object":
        return "automation", {"type": "object"}
    if format_type == "json_schema":
        schema_container = response_format.get("json_schema")
        schema = schema_container.get("schema") if isinstance(schema_container, dict) else None
        if not isinstance(schema, dict) or schema.get("type") != "object":
            raise ValueError("json_schema response_format must contain a top-level object schema.")
        try:
            Draft202012Validator.check_schema(schema)
        except SchemaError as error:
            raise ValueError(f"json_schema response_format is invalid: {error.message}") from error
        return "automation", schema
    raise ValueError(f"Unsupported response_format type: {format_type!r}")


def create_app(
    config: dict[str, Any] | None = None,
    client: OllamaClient | None = None,
) -> FastAPI:
    loaded_config = config or load_config()
    base_url = str(loaded_config["ollama"]["base_url"])
    timeout = float(loaded_config["ollama"].get("timeout_seconds", 180))
    active_client = client or OllamaClient(base_url, timeout)
    owns_client = client is None

    @asynccontextmanager
    async def lifespan(_: FastAPI) -> AsyncGenerator[None, None]:
        if owns_client:
            async with active_client:
                await initialize_runtime(loaded_config, active_client)
                yield
        else:
            await initialize_runtime(loaded_config, active_client)
            yield

    app = FastAPI(title="Local Agent Layer", version="0.2.0", lifespan=lifespan)
    app.state.orchestrator = Orchestrator(loaded_config, active_client)

    @app.get("/health")
    async def health() -> dict[str, str]:
        try:
            await active_client.list_models()
        except OllamaError as error:
            raise HTTPException(status_code=503, detail=str(error)) from error
        return {"status": "ok", "ollama": base_url}

    @app.post("/chat")
    async def chat(request: ChatRequest) -> StreamingResponse:
        async def events() -> AsyncIterator[str]:
            try:
                async for event in app.state.orchestrator.stream(
                    request.prompt,
                    task=request.task,
                    history=request.history,
                    tool_schema=request.tool_schema,
                    conversation_id=request.conversation_id,
                ):
                    yield sse(event.kind, event.data)
            except (OllamaError, ValueError) as error:
                yield sse("error", {"error": str(error)})

        return StreamingResponse(
            events(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
        )

    @app.post("/v1/chat/completions")
    async def openai_chat(request: OpenAIChatRequest) -> Response:
        try:
            prompt, history = openai_messages(request.messages)
            task, tool_schema = response_format_options(request.response_format)
        except ValueError as error:
            raise HTTPException(status_code=400, detail=str(error)) from error
        generation_options: dict[str, Any] = {}
        if request.temperature is not None:
            generation_options["temperature"] = request.temperature
        if request.max_tokens is not None:
            generation_options["num_predict"] = request.max_tokens

        request_id = f"chatcmpl-{uuid.uuid4().hex}"
        model_name = request.model or "local-agent-layer"
        created = int(time.time())

        async def response_events() -> AsyncIterator[str]:
            try:
                async for event in app.state.orchestrator.stream(
                    prompt,
                    task=task,
                    history=history,
                    tool_schema=tool_schema,
                    generation_options=generation_options,
                ):
                    if event.kind == "token":
                        yield "data: " + json.dumps({
                            "id": request_id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model_name,
                            "choices": [{
                                "index": 0,
                                "delta": {"content": event.data["content"]},
                                "finish_reason": None,
                            }],
                        }, ensure_ascii=False) + "\n\n"
                    elif event.kind == "error":
                        yield "data: " + json.dumps({"error": event.data["error"]}) + "\n\n"
                yield "data: " + json.dumps({
                    "id": request_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model_name,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                }) + "\n\n"
                yield "data: [DONE]\n\n"
            except (OllamaError, ValueError) as error:
                yield "data: " + json.dumps({"error": str(error)}) + "\n\n"
                yield "data: [DONE]\n\n"

        if request.stream:
            return StreamingResponse(
                response_events(),
                media_type="text/event-stream",
                headers={"Cache-Control": "no-cache"},
            )

        parts: list[str] = []
        try:
            async for event in app.state.orchestrator.stream(
                prompt,
                task=task,
                history=history,
                tool_schema=tool_schema,
                generation_options=generation_options,
            ):
                if event.kind == "token":
                    parts.append(str(event.data["content"]))
                elif event.kind == "error":
                    raise OllamaError(str(event.data["error"]))
        except (OllamaError, ValueError) as error:
            raise HTTPException(status_code=502, detail=str(error)) from error
        return JSONResponse({
            "id": request_id,
            "object": "chat.completion",
            "created": created,
            "model": model_name,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "".join(parts)},
                "finish_reason": "stop",
            }],
        })

    return app


app = create_app()
