# Phase 3: Warm-up, keep-alive, and memory guard

## Delivered

- Added toggleable startup warm-up, keep-alive, and memory guard settings in
  [config.yaml](./config.yaml).
- Fast-profile startup checks installed and resident models and warms installed
  profile models with their configured agent system prompt and context size.
  Missing models are warned about and skipped.
- Warm-up estimates total resident model size from Ollama's model metadata and
  skips if the configured 16 GiB budget would be exceeded, a resident model
  size is unknown, or the heavy model is already loaded. It does not unload
  models. Ollama's model size is an estimate, not a measurement of process or
  shared-GPU memory.
- Startup checks `ollama ps` and logs a warning for models reported CPU-only.
- Agent requests now apply profile `keep_alive`; heavy requests send their
  configured `think` level. The existing system prompt remains static and
  byte-identical across calls.
- Existing per-agent `num_ctx` and `num_predict` values were retained. Lowering
  these can truncate context or output, so they were not changed without
  evidence and approval for that quality trade-off.

## Verification and measurements

| Check | Result |
|---|---:|
| Unit tests | 19 passed |
| Python syntax checks | 13 files passed |
| Configured-context startup warm-up, Qwen 7B | 3.985 s wall-clock |
| Benchmark prompts | 20 |
| Benchmark PROCESSOR | `100% GPU` |
| Automation JSON validity | 4/4 baseline, 4/4 layer |

See [phase3-benchmark-report.md](./phase3-benchmark-report.md) for prompt-level
and aggregate comparison. In this single steady-state run, TTFT was 40.8% lower
and total latency was 2.8% lower with profile keep-alive; token throughput was
5.2% lower, and mean observed cold-load duration was 66.0% higher. Because Qwen
was already resident and the sample is order/cache-sensitive, this is mixed
evidence, not a general speedup claim.

## Limitations and safety

- `llama3.2:3b` is not installed, so its warm-up, GPU offload, voice/summarize
  performance, and the complete FAST profile were not verified.
- `gpt-oss:20b` was not loaded or invoked. No heavy-model swaps, unloads, or
  tool-call validation were performed.
- Heavy model `think` levels are now wired from config but cannot be verified
  without loading/invoking the heavy model.
