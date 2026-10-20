"""Compare default streaming Ollama requests with the configured keep-alive path."""

from __future__ import annotations

import argparse
import asyncio
import json
import statistics
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

from .client import OllamaClient, OllamaError, StreamResult

ROOT = Path(__file__).resolve().parent
AUTOMATION_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "action": {"type": "string"},
        "arguments": {"type": "object"},
    },
    "required": ["action", "arguments"],
    "additionalProperties": False,
}
PROMPTS: list[tuple[str, str, str]] = [
    ("chat", "What is the difference between RAM and storage? Answer in two sentences.", "chat"),
    ("chat", "Explain why the sky appears blue in one short paragraph.", "chat"),
    ("chat", "Give three practical ways to reduce distractions while working.", "chat"),
    ("chat", "What does an HTTP 404 response mean? Answer briefly.", "chat"),
    ("voice", "Tell me a quick encouraging message before a meeting.", "voice"),
    ("voice", "In one or two short sentences, explain what a checksum does.", "voice"),
    ("voice", "Give a brief spoken-style answer: what is a calendar reminder useful for?", "voice"),
    ("voice", "Say one concise sentence reminding me to take a short break.", "voice"),
    ("automation", 'Return JSON for action "open_app" with arguments appName "Calculator".', "automation"),
    ("automation", 'Return JSON for action "open_website" with arguments url "https://example.com".', "automation"),
    ("automation", 'Return JSON for action "search_web" with arguments query "weather" and engine "google".', "automation"),
    ("automation", 'Return JSON for action "remember_user_fact" with arguments category "preference" and text "likes concise replies".', "automation"),
    ("code", "Write a Python function that returns the larger of two integers.", "code"),
    ("code", "Show a JavaScript function that removes duplicate values from an array.", "code"),
    ("code", "Explain this Python error in one sentence: KeyError: 'name'.", "code"),
    ("code", "Write a SQL query that selects the five newest rows from a table named events.", "code"),
    ("summarize", "Summarize in one sentence: The service caches responses for five minutes, retries transient network failures once, and logs the final status.", "summarize"),
    ("summarize", "Summarize in one sentence: A user selects a file, reviews the proposed changes, approves them, and can restore the backup if needed.", "summarize"),
    ("complex", "A task has independent steps A and B, and step C needs both. Describe a safe execution order and why.", "complex"),
    ("complex", "A local app has high response latency. Give a short, ordered diagnostic plan that distinguishes model loading, prompt evaluation, and token generation.", "complex"),
]


@dataclass
class Measurement:
    category: str
    prompt: str
    baseline: StreamResult | None
    layer: StreamResult | None
    error: str | None
    json_ok_baseline: bool | None
    json_ok_layer: bool | None


def load_config(path: Path) -> dict[str, Any]:
    try:
        value = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as error:
        raise RuntimeError(f"Could not load config {path}: {error}") from error
    if not isinstance(value, dict):
        raise RuntimeError(f"Config {path} must contain a YAML mapping.")
    return value


def resolve_model(config: dict[str, Any], dotted_key: str) -> str:
    parts = dotted_key.split(".")
    value: Any = config
    for part in parts:
        if not isinstance(value, dict) or part not in value:
            raise RuntimeError(f"Missing model configuration key: {dotted_key}")
        value = value[part]
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"Model configuration {dotted_key} must be a non-empty string.")
    return value


def json_valid(category: str, result: StreamResult | None) -> bool | None:
    if category != "automation" or result is None:
        return None
    try:
        value = json.loads(result.text)
    except json.JSONDecodeError:
        return False
    return (
        isinstance(value, dict)
        and isinstance(value.get("action"), str)
        and isinstance(value.get("arguments"), dict)
    )


def seconds(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.3f}"


def mean(values: list[float]) -> float | None:
    return statistics.mean(values) if values else None


def metric_rows(results: list[Measurement]) -> list[str]:
    rows = [
        "| # | Task | Baseline TTFT (s) | Layer TTFT (s) | Baseline tok/s | Layer tok/s | Baseline total (s) | Layer total (s) | Cold load (base/layer, s) | JSON valid (base/layer) |",
        "|---:|---|---:|---:|---:|---:|---:|---:|---:|:---:|",
    ]
    for index, item in enumerate(results, start=1):
        baseline = item.baseline
        layer = item.layer
        if item.error:
            rows.append(
                f"| {index} | {item.category} | n/a | n/a | n/a | n/a | n/a | n/a | n/a | {item.error} |"
            )
            continue
        json_status = (
            f"{'yes' if item.json_ok_baseline else 'no'}/"
            f"{'yes' if item.json_ok_layer else 'no'}"
            if item.category == "automation"
            else "n/a"
        )
        rows.append(
            f"| {index} | {item.category} | "
            f"{seconds(baseline.time_to_first_token_seconds if baseline else None)} | "
            f"{seconds(layer.time_to_first_token_seconds if layer else None)} | "
            f"{seconds(baseline.tokens_per_second if baseline else None)} | "
            f"{seconds(layer.tokens_per_second if layer else None)} | "
            f"{seconds(baseline.total_latency_seconds if baseline else None)} | "
            f"{seconds(layer.total_latency_seconds if layer else None)} | "
            f"{seconds(baseline.load_duration_seconds if baseline else None)}/"
            f"{seconds(layer.load_duration_seconds if layer else None)} | {json_status} |"
        )
    return rows


def build_report(
    *,
    results: list[Measurement],
    model: str,
    processor: str,
    installed_models: list[str],
    loaded_models: list[str],
    elapsed: float,
    configured_models: list[str],
) -> str:
    successful = [row for row in results if row.baseline and row.layer]
    def metric(which: str, field: str) -> float | None:
        return mean([
            getattr(getattr(item, which), field)
            for item in successful
            if getattr(item, which) is not None
            and getattr(getattr(item, which), field) is not None
        ])

    base_ttft = metric("baseline", "time_to_first_token_seconds")
    layer_ttft = metric("layer", "time_to_first_token_seconds")
    base_total = metric("baseline", "total_latency_seconds")
    layer_total = metric("layer", "total_latency_seconds")
    base_tps = metric("baseline", "tokens_per_second")
    layer_tps = metric("layer", "tokens_per_second")
    base_steady = mean([
        row.baseline.total_latency_seconds - row.baseline.load_duration_seconds
        for row in successful if row.baseline is not None
    ])
    layer_steady = mean([
        row.layer.total_latency_seconds - row.layer.load_duration_seconds
        for row in successful if row.layer is not None
    ])
    baseline_cold_loads = [
        row.baseline.load_duration_seconds
        for row in results
        if row.baseline and row.baseline.load_duration_seconds > 0.05
    ]
    layer_cold_loads = [
        row.layer.load_duration_seconds
        for row in results
        if row.layer and row.layer.load_duration_seconds > 0.05
    ]
    base_load = mean(baseline_cold_loads)
    layer_load = mean(layer_cold_loads)
    automation = [row for row in results if row.category == "automation"]
    base_json = sum(row.json_ok_baseline is True for row in automation)
    layer_json = sum(row.json_ok_layer is True for row in automation)
    cold_loads = [
        (index, side, value.load_duration_seconds)
        for index, item in enumerate(results, start=1)
        for side, value in (("baseline", item.baseline), ("layer", item.layer))
        if value and value.load_duration_seconds > 0.05
    ]
    missing_models = sorted(set(configured_models) - set(installed_models))
    lines = [
        "# Phase 1 Ollama benchmark",
        "",
        f"- Model: `{model}`",
        f"- Installed models: {', '.join(f'`{item}`' for item in installed_models) or 'none reported'}",
        f"- Loaded models before benchmark: {', '.join(f'`{item}`' for item in loaded_models) or 'none'}",
        f"- `ollama ps` PROCESSOR: `{processor}`",
        f"- Prompts: {len(PROMPTS)} fixed; both paths stream and use the same model and prompt.",
        "- Baseline omits `keep_alive`; layer uses the configured profile `keep_alive` value. No warm-up, model swapping, prompt rewriting, or output cap is applied in Phase 1.",
        "- To reduce exact-prompt KV-cache order bias, the baseline runs in fixed order and the layer runs in reverse order.",
        "- All 20 prompts use the configured benchmark model to keep comparisons apples-to-apples; Phase 1 does not test agent-specific model routing.",
        f"- Configured profile models missing from `ollama list`: {', '.join(f'`{name}`' for name in missing_models) or 'none'}.",
        f"- Wall-clock benchmark duration: {elapsed:.1f}s.",
        "",
        *metric_rows(results),
        "",
        "## Aggregate",
        "",
        "| Metric | Baseline mean | Layer mean | Relative layer change |",
        "|---|---:|---:|---:|",
    ]
    for label, baseline, layer in [
        ("TTFT (s)", base_ttft, layer_ttft),
        ("Total latency (s)", base_total, layer_total),
        ("Generation (tokens/s)", base_tps, layer_tps),
        ("Mean cold load when observed (s)", base_load, layer_load),
        ("Steady-state latency (s)", base_steady, layer_steady),
    ]:
        change = (
            "n/a"
            if baseline is None or layer is None or baseline == 0
            else f"{(layer - baseline) / baseline * 100:+.1f}%"
        )
        lines.append(f"| {label} | {seconds(baseline)} | {seconds(layer)} | {change} |")
    lines.extend([
        "",
        f"- Automation JSON validity: baseline {base_json}/{len(automation)}, layer {layer_json}/{len(automation)}.",
        "- Cold-load observations (`load_duration` > 50ms): "
        + (
            ", ".join(
                f"prompt {index} {side} {duration:.3f}s"
                for index, side, duration in cold_loads
            )
            if cold_loads
            else "none observed; report steady-state results separately from cold-load penalty."
        ),
        "",
        "## Environment and skipped checks",
        "",
        "- GPU/CPU split is reported from `ollama ps`; it reflects the loaded benchmark model, not models that were not loaded.",
        "- This harness does not load or unload the heavy profile. Heavy-model JSON/tool-call and GPU-offload checks remain pending explicit approval to swap the heavy model.",
        "- FAST profile models absent from `ollama list` are not benchmarked in Phase 1; the benchmark uses the configured `fast.chat` model for all task categories as a safe baseline.",
        "- Optimization verdict: compare aggregate table. If latency or throughput is not better, keep-alive did not demonstrate a speedup in this run.",
    ])
    return "\n".join(lines) + "\n"


async def ollama_cli(command: str) -> str:
    try:
        process = await asyncio.create_subprocess_exec(
            "ollama", command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
    except OSError as error:
        raise RuntimeError(f"Could not run `ollama {command}`: {error}") from error
    stdout, stderr = await process.communicate()
    if process.returncode != 0:
        raise RuntimeError(
            f"`ollama {command}` failed: {stderr.decode(errors='replace').strip()}"
        )
    return stdout.decode(errors="replace")


def parse_cli_table(output: str) -> tuple[list[str], dict[str, str]]:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if len(lines) < 2:
        return [], {}
    rows = [line.split() for line in lines[1:]]
    model_names = [row[0] for row in rows if row]
    model_columns: dict[str, str] = {}
    for row in rows:
        if len(row) < 5:
            continue
        context_index = next(
            (index for index, token in enumerate(row[4:], start=4) if token.isdigit()),
            None,
        )
        if context_index is not None:
            model_columns[row[0]] = " ".join(row[4:context_index])
    return model_names, model_columns


async def run_benchmark(config_path: Path, report_path: Path) -> None:
    config = load_config(config_path)
    ollama_config = config.get("ollama", {})
    benchmark = config.get("benchmark", {})
    profiles = config.get("profiles", {})
    agents = config.get("agents", {})
    model = resolve_model(config, benchmark["baseline_model_key"])
    agent = agents[benchmark["optimized_agent"]]
    keep_alive = profiles[agent["profile"]]["keep_alive"]
    base_url = str(ollama_config["base_url"])
    timeout = float(benchmark.get("timeout_seconds", ollama_config.get("timeout_seconds", 180)))
    installed_output, ps_output = await asyncio.gather(
        ollama_cli("list"), ollama_cli("ps")
    )
    installed, _ = parse_cli_table(installed_output)
    loaded, processors = parse_cli_table(ps_output)
    if model not in installed:
        raise RuntimeError(
            f"Benchmark model {model!r} is missing. Pull it manually; this tool will not install models."
        )
    if model not in loaded:
        raise RuntimeError(
            f"Benchmark model {model!r} is not already loaded. To avoid model swaps, load it manually and rerun."
        )
    processor = processors.get(model, "PROCESSOR column unavailable")
    if "CPU" in processor.upper() and "GPU" not in processor.upper():
        print(
            f"WARNING: ollama ps reports {model} on {processor}, not GPU. "
            "Performance measurements may not represent GPU offload.",
            file=sys.stderr,
        )

    results: list[Measurement] = []
    start = time.perf_counter()
    async with OllamaClient(base_url, timeout) as client:
        measurements = [
            Measurement(category, prompt, None, None, None, None, None)
            for category, prompt, _ in PROMPTS
        ]
        for index, (category, prompt, agent_name) in enumerate(PROMPTS):
            agent_config = agents[agent_name]
            system = str(agent_config["system"])
            messages = [{"role": "system", "content": system}, {"role": "user", "content": prompt}]
            output_format = AUTOMATION_SCHEMA if category == "automation" else None
            try:
                measurements[index].baseline = await client.stream_chat(
                    model=model,
                    messages=messages,
                    response_format=output_format,
                )
            except OllamaError as failure:
                measurements[index].error = f"Baseline: {failure}"
            print(
                f"baseline {index + 1:02}/{len(PROMPTS)} {category}: "
                f"TTFT {seconds(measurements[index].baseline.time_to_first_token_seconds if measurements[index].baseline else None)}s"
            )

        for index in reversed(range(len(PROMPTS))):
            category, prompt, agent_name = PROMPTS[index]
            agent_config = agents[agent_name]
            system = str(agent_config["system"])
            messages = [{"role": "system", "content": system}, {"role": "user", "content": prompt}]
            output_format = AUTOMATION_SCHEMA if category == "automation" else None
            try:
                measurements[index].layer = await client.stream_chat(
                    model=model,
                    messages=messages,
                    keep_alive=keep_alive,
                    response_format=output_format,
                )
            except OllamaError as failure:
                previous = measurements[index].error
                measurements[index].error = (
                    f"{previous}; Layer: {failure}" if previous else f"Layer: {failure}"
                )
            measurements[index].json_ok_baseline = json_valid(
                category, measurements[index].baseline
            )
            measurements[index].json_ok_layer = json_valid(
                category, measurements[index].layer
            )
            print(
                f"layer   {index + 1:02}/{len(PROMPTS)} {category}: "
                f"TTFT {seconds(measurements[index].layer.time_to_first_token_seconds if measurements[index].layer else None)}s"
            )
        results.extend(measurements)
    report = build_report(
        results=results,
        model=model,
        processor=processor,
        installed_models=installed,
        loaded_models=loaded,
        elapsed=time.perf_counter() - start,
        configured_models=[
            str(model_name)
            for profile in profiles.values()
            for model_name in (
                list(profile.get("models", {}).values())
                if isinstance(profile.get("models"), dict)
                else [profile["model"]] if "model" in profile else []
            )
        ],
    )
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(report, encoding="utf-8")
    print(f"\nReport written to {report_path}")
    print(report)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=ROOT / "config.yaml")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    try:
        config = load_config(args.config)
        configured_report = Path(config["benchmark"]["report_path"])
        report_path = args.report or (
            configured_report if configured_report.is_absolute() else ROOT / configured_report
        )
        asyncio.run(run_benchmark(args.config, report_path))
    except (KeyError, TypeError, ValueError, RuntimeError) as error:
        print(f"Benchmark failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
