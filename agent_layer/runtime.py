"""Safe startup warm-up and model-residency checks."""

from __future__ import annotations

import asyncio
import logging
import time
from typing import Any

from .bench import parse_cli_table
from .client import OllamaClient, OllamaError

logger = logging.getLogger(__name__)
GIB = 1024**3


async def _ollama_ps() -> tuple[list[str], dict[str, str]]:
    try:
        process = await asyncio.create_subprocess_exec(
            "ollama",
            "ps",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
    except OSError as error:
        raise RuntimeError(f"Could not run `ollama ps`: {error}") from error
    stdout, stderr = await process.communicate()
    if process.returncode != 0:
        detail = stderr.decode(errors="replace").strip()
        raise RuntimeError(f"`ollama ps` failed: {detail or process.returncode}")
    return parse_cli_table(stdout.decode(errors="replace"))


def _model_name(model: dict[str, Any]) -> str:
    return str(model.get("name", ""))


def _warmup_prompt(config: dict[str, Any], model_name: str) -> tuple[str, int]:
    for agent in config.get("agents", {}).values():
        profile = config["profiles"][agent["profile"]]
        model_key = agent["model_key"]
        configured_model = (
            profile["models"][model_key] if "models" in profile else profile[model_key]
        )
        if str(configured_model) == model_name:
            return str(agent["system"]), int(agent["num_ctx"])
    return "Warm up the configured local model.", 512


async def initialize_runtime(config: dict[str, Any], client: OllamaClient) -> None:
    """Report residency and warm only an installed, memory-safe fast profile."""
    optimizations = config.get("optimizations", {})
    try:
        loaded = await client.loaded_models()
        installed = await client.list_models()
    except OllamaError as error:
        logger.warning("Ollama startup check unavailable; startup warm-up skipped: %s", error)
        return

    loaded_names = {_model_name(model) for model in loaded}
    installed_by_name = {
        _model_name(model): model for model in installed if _model_name(model)
    }
    logger.info("Ollama models loaded at startup: %s", ", ".join(sorted(loaded_names)) or "none")

    try:
        _, processors = await _ollama_ps()
        for model_name in sorted(loaded_names):
            processor = processors.get(model_name)
            if processor and "CPU" in processor.upper() and "GPU" not in processor.upper():
                logger.warning(
                    "Ollama reports %s on %s rather than GPU; performance may be reduced.",
                    model_name,
                    processor,
                )
    except RuntimeError as error:
        logger.warning("Could not verify GPU offload at startup: %s", error)

    if not optimizations.get("startup_warmup", True):
        return

    fast = config["profiles"]["fast"]
    targets = list(dict.fromkeys(str(name) for name in fast["models"].values()))
    missing = [name for name in targets if name not in installed_by_name]
    for name in missing:
        logger.warning("Fast-profile model %s is not installed; skipping its warm-up.", name)

    heavy_model = str(config["profiles"]["heavy"]["model"])
    if heavy_model in loaded_names:
        logger.warning(
            "Heavy model %s is already loaded; skipping fast-profile warm-up to avoid memory pressure.",
            heavy_model,
        )
        return

    warm_targets = [name for name in targets if name in installed_by_name]
    resident_names = loaded_names | set(warm_targets)
    sizes: dict[str, int] = {}
    for name in resident_names:
        record = installed_by_name.get(name)
        if record is not None:
            try:
                sizes[name] = int(record["size"])
            except (KeyError, TypeError, ValueError):
                continue

    unknown_sizes = [name for name in warm_targets if name not in sizes]
    for name in unknown_sizes:
        logger.warning("Model size for %s is unavailable; skipping its warm-up for safety.", name)
    warm_targets = [name for name in warm_targets if name in sizes]
    unknown_resident_sizes = sorted(
        name for name in loaded_names - set(sizes)
    )
    if unknown_resident_sizes and optimizations.get("memory_guard", True):
        logger.warning(
            "Fast-profile warm-up skipped because the size of already-loaded model(s) "
            "is unknown: %s.",
            ", ".join(unknown_resident_sizes),
        )
        return
    estimated_gb = sum(sizes.get(name, 0) for name in resident_names) / GIB
    budget_gb = float(optimizations.get("memory_budget_gb", 16))
    if optimizations.get("memory_guard", True) and estimated_gb > budget_gb:
        logger.warning(
            "Fast-profile warm-up skipped: Ollama model-size estimate %.2f GiB exceeds "
            "the %.2f GiB configured budget.",
            estimated_gb,
            budget_gb,
        )
        return

    for model_name in warm_targets:
        started = time.perf_counter()
        system_prompt, context_size = _warmup_prompt(config, model_name)
        try:
            result = await client.stream_chat(
                model=model_name,
                messages=[
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": "Ready."},
                ],
                options={"temperature": 0, "num_predict": 1, "num_ctx": context_size},
                keep_alive=(
                    fast.get("keep_alive")
                    if optimizations.get("keep_alive", True)
                    else None
                ),
            )
        except OllamaError as error:
            logger.warning("Warm-up failed for %s: %s", model_name, error)
            continue
        elapsed = time.perf_counter() - started
        logger.info(
            "Fast-profile model %s warm-up completed in %.3f s (Ollama load duration %.3f s).",
            model_name,
            elapsed,
            result.load_duration_seconds,
        )
