# Final benchmark summary

The same 20-prompt Qwen 7B harness compares a direct streamed call (baseline)
with a streamed call using the configured profile keep-alive. The runs below
were performed at different times and are not a controlled comparison against
one another; use the per-run reports for full prompt-level results.

| Run | PROCESSOR | Baseline / layer TTFT (s) | Baseline / layer total (s) | Baseline / layer tok/s | Baseline / layer mean cold load (s) | JSON valid |
|---|---|---:|---:|---:|---:|---:|
| Phase 1 | 100% GPU | 0.157 / 0.306 | 3.675 / 3.840 | 20.039 / 17.618 | n/a / 0.140 | 4/4, 4/4 |
| Phase 3 | 100% GPU | 0.439 / 0.260 | 4.898 / 4.761 | 16.869 / 15.987 | 0.100 / 0.166 | 4/4, 4/4 |
| Phase 4 | 100% GPU | 0.165 / 0.165 | 4.342 / 3.754 | 18.833 / 17.634 | 0.168 / 0.081 | 4/4, 4/4 |

Each baseline/layer pair comes from the same run. Results are mixed: TTFT and
total latency improved in some runs and regressed or stayed flat in others;
throughput was lower for the layer in all three runs. The Phase 4 run had lower
total latency and observed cold-load duration, but lower generation throughput.
Prompt order, prior model residency, server cache state, and background load can
materially shift results. The benchmarks do not demonstrate a stable,
general-purpose speedup from keep-alive.

## Additional measured checks

- Deterministic rule routing: 10,000 requests, mean 0.0028 ms, p95 0.0035 ms,
  maximum 0.4217 ms. This measures routing alone, not Ollama generation.
- In-memory cache lookup: 10,000 hits, mean 0.610 µs, p95 0.500 µs. This is
  local cache overhead, not end-to-end response latency.
- The live OpenAI-compatible SSE smoke test returned HTTP 200 and terminated
  with `[DONE]` against the already-loaded Qwen model.

## Coverage limits

- All benchmark prompts use `qwen2.5:7b` to compare the two request paths
  consistently. The configured `llama3.2:3b` was not installed, so voice,
  summarize, router-model, and complete FAST-profile behavior were not measured.
- The Phase 4 benchmark does not measure planner, memory-compression, or cache
  inference gains. Planning and compression require the already-loaded 3B model;
  compression and caching are disabled by default.
- `gpt-oss:20b` was not loaded, unloaded, swapped, or invoked. Heavy-model
  JSON/tool-call validity and GPU split remain unverified pending authorization
  to use that model.
- The Phase 4 benchmark's PROCESSOR reading was `100% GPU` for Qwen; it does not
  establish GPU offload for any model that was not loaded.

Detailed results: [Phase 1](./benchmark-report.md), [Phase 3](./phase3-benchmark-report.md),
and [Phase 4](./phase4-benchmark-report.md).
