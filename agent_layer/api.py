"""FastAPI streaming API for local Ollama agents."""

from __future__ import annotations

import json
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, AsyncIterator

import yaml
from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
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
    async def lifespan(_: FastAPI) -> AsyncIterator[None]:
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
                ):
                    yield sse(event.kind, event.data)
            except (OllamaError, ValueError) as error:
                yield sse("error", {"error": str(error)})

        return StreamingResponse(
            events(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
        )

    return app


app = create_app()
