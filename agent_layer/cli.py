"""Command-line entry point for the local agent layer."""

from __future__ import annotations

import argparse
import asyncio
import sys

from .api import load_config
from .client import OllamaClient, OllamaError
from .orchestrator import Orchestrator


async def run(prompt: str, task: str | None) -> int:
    config = load_config()
    async with OllamaClient(
        str(config["ollama"]["base_url"]),
        float(config["ollama"].get("timeout_seconds", 180)),
    ) as client:
        orchestrator = Orchestrator(config, client)
        try:
            async for event in orchestrator.stream(prompt, task=task):
                if event.kind == "meta":
                    print(f"[task={event.data['task']} agent={event.data['agent']}]", file=sys.stderr)
                elif event.kind == "token":
                    print(event.data["content"], end="", flush=True)
                elif event.kind == "error":
                    print(f"\nError: {event.data['error']}", file=sys.stderr)
                    return 1
            print()
        except (OllamaError, ValueError) as error:
            print(f"Agent request failed: {error}", file=sys.stderr)
            return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("prompt", help="Request to route to a local agent")
    parser.add_argument(
        "--task",
        choices=("chat", "voice", "automation", "code", "summarize", "complex"),
        help="Explicitly select a task; otherwise use the router",
    )
    args = parser.parse_args()
    return asyncio.run(run(args.prompt, args.task))


if __name__ == "__main__":
    raise SystemExit(main())
