# Phase 2: Router, agents, and streaming API

## Delivered

- Added keyword-first routing for chat, voice, automation, code, summarize, and
  complex requests. The router uses the configured 3B model with a strict JSON
  schema only for ambiguous requests when that model is installed.
- Added config-driven chat, voice, automation, code, summarize, and heavy agents.
  Voice output is emitted at sentence boundaries; automation responses are
  parsed and JSON-Schema validated, with one retry on invalid output.
- Added the FastAPI `/chat` SSE endpoint, `/health`, and a CLI entry point.
- Added a guard that uses the heavy model only if it is already loaded. Phase 2
  does not load or unload models.

## Measurements and verification

| Check | Result |
|---|---:|
| Rule-router calls, fixed code-routed request | 10,000 |
| Rule-router mean latency | 0.0028 ms |
| Rule-router p95 latency | 0.0035 ms |
| Rule-router max latency | 0.4217 ms |
| Chat-agent smoke test, Qwen 7B TTFT | 0.502 s |
| Automated tests | 14 passed |
| Python syntax checks | 11 files passed |

The router timing excludes Ollama calls; it verifies the deterministic rule path
comfortably meets the 150 ms target. The single chat smoke test is not an
apples-to-apples speed comparison, so no speedup is claimed.

## Model and benchmark limitations

- `qwen2.5:7b` was the only loaded model; `ollama ps` reported `100% GPU`.
- `llama3.2:3b` is not installed. Ambiguous routing therefore safely falls back
  to chat. Voice and summarize are configured to use the 3B model and cannot be
  meaningfully benchmarked until it is available.
- `gpt-oss:20b` is installed but was not loaded or invoked. No heavy-model swap,
  JSON/tool-call validation, or heavy-model GPU split was attempted.
- A full 20-prompt agent benchmark is deferred until the configured 3B model is
  available. The Phase 1 benchmark remains the measured Qwen baseline in
  [benchmark-report.md](./benchmark-report.md).

No latency improvement is claimed for the layer based on the Phase 2 checks.
