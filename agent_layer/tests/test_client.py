from __future__ import annotations

import json
import unittest

import httpx

from bench import parse_cli_table
from client import OllamaClient, OllamaError


class OllamaClientTests(unittest.IsolatedAsyncioTestCase):
    def test_ollama_ps_parser_keeps_processor_split_together(self) -> None:
        output = (
            "NAME       ID       SIZE    PROCESSOR  CONTEXT  RUNNER   UNTIL\n"
            "test-model abc123   4.8 GB  100% GPU   4096     llamacpp 3 minutes\n"
        )
        names, processors = parse_cli_table(output)
        self.assertEqual(names, ["test-model"])
        self.assertEqual(processors["test-model"], "100% GPU")

    async def test_stream_chat_collects_content_timing_and_final_metrics(self) -> None:
        events = [
            {"message": {"role": "assistant", "content": "Hello"}, "done": False},
            {
                "message": {"role": "assistant", "content": "!"},
                "done": True,
                "prompt_eval_count": 9,
                "eval_count": 3,
                "load_duration": 100_000_000,
                "eval_duration": 250_000_000,
            },
        ]
        transport = httpx.MockTransport(
            lambda request: httpx.Response(
                200,
                content="".join(json.dumps(event) + "\n" for event in events),
            )
        )
        async with httpx.AsyncClient(transport=transport) as http:
            async with OllamaClient("http://ollama.test", client=http) as ollama:
                result = await ollama.stream_chat(
                    model="configured-model",
                    messages=[{"role": "user", "content": "Hi"}],
                    keep_alive=-1,
                )

        self.assertEqual(result.text, "Hello!")
        self.assertIsNotNone(result.time_to_first_token_seconds)
        self.assertEqual(result.prompt_tokens, 9)
        self.assertEqual(result.generated_tokens, 3)
        self.assertAlmostEqual(result.load_duration_seconds, 0.1)
        self.assertAlmostEqual(result.tokens_per_second or 0, 12.0)

    async def test_stream_chat_rejects_incomplete_response(self) -> None:
        transport = httpx.MockTransport(
            lambda request: httpx.Response(
                200,
                content=json.dumps({"message": {"content": "partial"}, "done": False}),
            )
        )
        async with httpx.AsyncClient(transport=transport) as http:
            async with OllamaClient("http://ollama.test", client=http) as ollama:
                with self.assertRaisesRegex(OllamaError, "before a final response"):
                    await ollama.stream_chat(
                        model="configured-model",
                        messages=[{"role": "user", "content": "Hi"}],
                    )

    async def test_chat_stream_yields_tokens_and_rejects_server_error(self) -> None:
        async def handler(request: httpx.Request) -> httpx.Response:
            if request.url.path.endswith("/error/api/chat"):
                return httpx.Response(500, text="upstream failure")
            return httpx.Response(
                200,
                content=(
                    json.dumps({"message": {"content": "one"}, "done": False}) + "\n"
                    + json.dumps({"message": {"content": " two"}, "done": True}) + "\n"
                ),
            )

        transport = httpx.MockTransport(handler)
        async with httpx.AsyncClient(transport=transport) as http:
            async with OllamaClient("http://ollama.test", client=http) as ollama:
                chunks = [
                    chunk async for chunk in ollama.chat_stream(
                        model="configured-model",
                        messages=[{"role": "user", "content": "Hi"}],
                    )
                ]
                self.assertEqual(chunks, ["one", " two"])

                ollama.base_url = "http://ollama.test/error"
                with self.assertRaisesRegex(OllamaError, "HTTP 500"):
                    _ = [
                        chunk async for chunk in ollama.chat_stream(
                            model="configured-model",
                            messages=[{"role": "user", "content": "Hi"}],
                        )
                    ]


if __name__ == "__main__":
    unittest.main()
