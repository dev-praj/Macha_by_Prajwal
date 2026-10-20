# Phase 4: Planning, bounded memory, and deterministic cache

## Delivered

- Added a JSON-schema planner for complex requests. It runs only when the
  configured 3B router model is already loaded. Valid independent subtasks are
  dispatched with `asyncio.gather` behind the configured semaphore (default
  concurrency 2), then their results are combined by the fast chat agent.
- If the router model is absent/unloaded, planning is skipped. If the heavy
  model is resident, the layer does not load/run the small planner or parallel
  fast workers, avoiding a conflicting model profile.
- Added opt-in conversation-memory compression. It preserves the last N user
  turns, keeps system instructions, and maintains a rolling summary by
  `conversation_id`. Summarization is attempted only with an already-loaded
  summary model. When memory cannot be compressed safely, the request returns
  an explicit error rather than dropping history.
- Added an opt-in exact-match TTL cache for automation and summarize results,
  with bounded in-memory storage and optional SQLite persistence. Cache keys
  include prompt, history, schema, generation options, system prompt, and model.
  Chat and voice are never cached.
- Memory compression and response caching are disabled by default. These can
  affect context fidelity or behavior and remain user-controlled.

## Verification and measurements

| Check | Result |
|---|---:|
| Automated tests | 39 passed |
| Python syntax checks | 18 files passed |
| In-memory cache hit loop | 10,000 hits |
| Cache hit mean / p95 | 0.610 µs / 0.500 µs |

The cache microbenchmark measures local lookup overhead only, not Ollama
inference latency. See [phase4-benchmark-report.md](./phase4-benchmark-report.md)
for the 20-prompt Qwen regression run: TTFT -0.1%, total latency -13.5%,
generation throughput -6.4%, and observed mean cold-load duration -51.7% for
the keep-alive request path. The run does not exercise planning or summarization,
and the changes are not evidence of those features improving inference speed.

## Limitations

- `llama3.2:3b` is not installed or resident, so live planner output and
  conversation compression could not be validated against Ollama. They are
  covered with mocks and remain safe no-op/error paths until the model is
  manually installed and loaded.
- Heavy-model planning and parallel work remain intentionally skipped to avoid
  loading fast models beside the heavy model. No heavy model was loaded,
  unloaded, swapped, or invoked.
- Model-specific quality and latency effects remain unmeasured. No speedup is
  claimed for planner or memory compression.
