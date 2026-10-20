# Local Ollama Agent Layer

Async, config-driven agent routing and streaming on top of a local Ollama
server. It includes task-specific agents, a rule-first router, an SSE API, a
CLI, a bounded optional planner, opt-in history compression, and a deterministic
response cache.

## Requirements and setup

- Python 3.11 or newer.
- Ollama reachable at the `ollama.base_url` configured in [config.yaml](./config.yaml).
- Install dependencies from the repository root:

  ```powershell
  python -m pip install -r agent_layer\requirements.txt
  ```

- The configured FAST profile uses `llama3.2:3b` and `qwen2.5:7b`; the HEAVY
  profile uses `gpt-oss:20b`. Install desired models manually with Ollama.
  This application does not pull models automatically.
- Start the API from the repository root:

  ```powershell
  python -m uvicorn agent_layer.api:app --host 127.0.0.1 --port 8000
  ```

  Startup checks model residency, warns about missing models or CPU-only
  offload, and warms installed FAST models only if the model-size estimate is
  within the configured budget and the heavy model is not resident. Warm-up
  never unloads a model.

Run a prompt from the CLI:

```powershell
python -m agent_layer.cli "Explain what a checksum does"
```

The CLI and API stream output; `/chat` emits SSE `meta`, `token`, `done`, and
`error` events. `/health` checks whether Ollama responds.

## API endpoints

- `POST /chat`: agent-layer request with `prompt`, optional `task`, `history`,
  `tool_schema`, and `conversation_id`.
- `POST /v1/chat/completions`: OpenAI-style chat completions, with streaming by
  default; accepts `messages` with string content, `stream`, `model`,
  `temperature`, and `max_tokens`.
- `response_format` supports `{"type":"json_object"}` and
  `{"type":"json_schema","json_schema":{"schema":{...}}}` for top-level object
  schemas. Other OpenAI features such as tool/function calling, multimodal
  messages, and non-string content parts are not implemented.
  `max_tokens` maps to `num_predict` and is capped by the selected agent's
  configured limit.

Point an OpenAI-compatible client at `http://127.0.0.1:8000/v1` and use a model
label such as `local-agent-layer`.

## Configuration reference

All model names, per-agent prompts and generation settings are in
[config.yaml](./config.yaml); model names are not hardcoded in agent classes.

- `ollama`: base URL and request timeout.
- `profiles.fast`: model per agent role and `keep_alive`; `profiles.heavy`:
  model, short keep-alive, and `think` level.
- `agents`: profile/model key, fixed system prompt, temperature,
  `num_predict`, and `num_ctx`.
- `optimizations`: keep-alive, startup warm-up, memory guard, and estimated
  memory budget. The guard uses Ollama model-size metadata as an estimate, not
  a direct measurement of shared GPU/OS memory.
- `planner`: max subtasks and concurrent subtasks. The planner runs only when
  the configured small router model is already loaded; without it, complex
  requests use a single fast-agent fallback. Heavy-model requests are not
  decomposed into concurrent fast-model work.
- `memory`: disabled by default. When enabled, older conversation turns are
  summarized only if the configured summarizer model is already loaded. A
  `conversation_id` retains a rolling summary in process memory. Token counting
  is an approximate character-based estimate; over-budget input that cannot
  safely be summarized returns an explicit error instead of dropping history.
- `response_cache`: disabled by default. Only automation and summarize results
  are eligible; chat and voice are never cached. Set `sqlite_path` to opt into
  SQLite persistence. Cache keys include the normalized prompt, history,
  schema, and generation overrides.

History compression is disabled by default to avoid silently trading context
fidelity for speed. The optional cache and planner also remain configurable;
measure your use case before enabling them.

## Benchmarking

The 20-prompt harness compares direct Ollama calls with the configured
keep-alive request path. It requires the configured benchmark model to be
installed and already loaded; it does not load or unload models.

```powershell
python -m agent_layer.bench --report agent_layer\benchmark-report.md
```

Reports include per-prompt and aggregate time-to-first-token, tokens/sec, total
latency, JSON validity, cold-load observations, and `ollama ps` processor
split. Results vary with cache state, prompt ordering, GPU load, power mode, and
background applications. A single run is not proof of a general speedup.

Print optional Ollama server recommendations without applying them:

```powershell
python -m agent_layer.recommend_ollama_env
```

The script explains `OLLAMA_FLASH_ATTENTION=1`,
`OLLAMA_KV_CACHE_TYPE=q8_0`, `OLLAMA_NUM_PARALLEL=1`, and
`OLLAMA_MAX_LOADED_MODELS=2`. Increasing parallelism or loaded models increases
KV-cache/shared-memory use and may reduce throughput or cause thrashing. The
two-model recommendation is for the FAST profile only; do not co-load the
13–14 GB HEAVY model alongside fast models within the 16 GB shared-memory
budget.

On Windows, use the best-performance power mode while plugged in and close
heavy background applications before benchmarking; both can measurably affect
tokens/sec.

## Final validation

The agent-layer suite passed 39 tests in the selected project virtual
environment. The CLI and environment-recommendation script were smoke-tested,
and a live `/v1/chat/completions` streaming request returned HTTP 200 and
`[DONE]` using Qwen 7B. Current residency was `100% GPU` for Qwen. The
configured 3B model remains unavailable, and the HEAVY model was not loaded or
invoked; see the [final benchmark limitations](./final-benchmark-report.md).

## Adding an agent

1. Add the model role under `profiles.fast.models` or a new profile in
   `config.yaml`; use a config key rather than embedding a model name in code.
2. Add the agent's `profile`, `model_key`, fixed `system` prompt,
   `temperature`, `num_predict`, and `num_ctx` under `agents`.
3. Add a class or reuse `Agent` in `agents.py`, then register it in
   `create_agents`.
4. Add the task label to `router.py` if it should be rule-routed, and connect
   the agent in `Orchestrator` if it needs special behavior.
5. Add mocked unit tests under `tests/`; do not benchmark by swapping a model
   into or out of a running profile.

## Phase results

- [Phase 1 benchmark](./benchmark-report.md)
- [Phase 2 report](./phase2-report.md)
- [Phase 3 report](./phase3-report.md) and [Phase 3 benchmark](./phase3-benchmark-report.md)
- [Phase 4 report](./phase4-report.md) and [Phase 4 benchmark](./phase4-benchmark-report.md)
- [Final benchmark summary](./final-benchmark-report.md)
