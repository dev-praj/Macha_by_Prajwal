# Phase 1 Ollama benchmark

- Model: `qwen2.5:7b`
- Installed models: `agent-general:latest`, `agent-summarizer:latest`, `agent-coder:latest`, `agent-router:latest`, `gpt-oss:20b`, `gemma4:26b`, `qwen2.5:7b`
- Loaded models before benchmark: `qwen2.5:7b`
- `ollama ps` PROCESSOR: `100% GPU`
- Prompts: 20 fixed; both paths stream and use the same model and prompt.
- Baseline omits `keep_alive`; layer uses the configured profile `keep_alive` value. No warm-up, model swapping, prompt rewriting, or output cap is applied in Phase 1.
- To reduce exact-prompt KV-cache order bias, the baseline runs in fixed order and the layer runs in reverse order.
- All 20 prompts use the configured benchmark model to keep comparisons apples-to-apples; Phase 1 does not test agent-specific model routing.
- Configured profile models missing from `ollama list`: `llama3.2:3b`.
- Wall-clock benchmark duration: 150.4s.

| # | Task | Baseline TTFT (s) | Layer TTFT (s) | Baseline tok/s | Layer tok/s | Baseline total (s) | Layer total (s) | Cold load (base/layer, s) | JSON valid (base/layer) |
|---:|---|---:|---:|---:|---:|---:|---:|---:|:---:|
| 1 | chat | 0.211 | 0.356 | 20.066 | 18.110 | 2.904 | 3.105 | 0.011/0.223 | n/a |
| 2 | chat | 0.092 | 0.285 | 19.924 | 17.601 | 2.652 | 3.290 | 0.007/0.142 | n/a |
| 3 | chat | 0.090 | 0.780 | 19.815 | 17.605 | 5.490 | 6.016 | 0.007/0.274 | n/a |
| 4 | chat | 0.100 | 0.226 | 20.740 | 19.250 | 1.017 | 1.040 | 0.009/0.115 | n/a |
| 5 | voice | 0.081 | 0.266 | 21.158 | 19.977 | 0.695 | 0.973 | 0.008/0.154 | n/a |
| 6 | voice | 0.082 | 0.285 | 20.831 | 18.566 | 0.995 | 1.257 | 0.008/0.171 | n/a |
| 7 | voice | 0.082 | 0.229 | 20.333 | 17.998 | 1.458 | 1.847 | 0.006/0.144 | n/a |
| 8 | voice | 0.088 | 0.280 | 21.752 | 19.971 | 0.501 | 0.619 | 0.010/0.063 | n/a |
| 9 | automation | 0.096 | 0.347 | 20.375 | 17.944 | 1.322 | 1.742 | 0.009/0.218 | yes/yes |
| 10 | automation | 0.095 | 0.240 | 20.057 | 17.823 | 1.491 | 1.810 | 0.006/0.114 | yes/yes |
| 11 | automation | 0.097 | 0.190 | 20.070 | 17.655 | 1.692 | 2.005 | 0.009/0.052 | yes/yes |
| 12 | automation | 0.095 | 0.355 | 19.980 | 18.033 | 1.895 | 2.343 | 0.006/0.121 | yes/yes |
| 13 | code | 0.364 | 0.470 | 20.632 | 18.266 | 1.480 | 1.728 | 0.007/0.030 | n/a |
| 14 | code | 0.094 | 0.266 | 20.643 | 15.972 | 1.056 | 1.518 | 0.008/0.122 | n/a |
| 15 | code | 0.080 | 0.118 | 20.619 | 18.274 | 1.100 | 0.830 | 0.006/0.021 | n/a |
| 16 | code | 0.370 | 0.167 | 18.655 | 15.090 | 1.439 | 1.491 | 0.010/0.042 | n/a |
| 17 | summarize | 0.152 | 0.143 | 20.600 | 14.575 | 1.112 | 1.588 | 0.016/0.033 | n/a |
| 18 | summarize | 0.089 | 0.231 | 20.337 | 16.384 | 1.368 | 1.822 | 0.006/0.024 | n/a |
| 19 | complex | 0.234 | 0.628 | 17.813 | 16.383 | 20.558 | 25.105 | 0.008/0.122 | n/a |
| 20 | complex | 0.548 | 0.267 | 16.377 | 16.885 | 23.266 | 16.676 | 0.022/0.064 | n/a |

## Aggregate

| Metric | Baseline mean | Layer mean | Relative layer change |
|---|---:|---:|---:|
| TTFT (s) | 0.157 | 0.306 | +95.2% |
| Total latency (s) | 3.675 | 3.840 | +4.5% |
| Generation (tokens/s) | 20.039 | 17.618 | -12.1% |
| Mean cold load when observed (s) | n/a | 0.140 | n/a |
| Steady-state latency (s) | 3.666 | 3.728 | +1.7% |

- Automation JSON validity: baseline 4/4, layer 4/4.
- Keep-alive verdict: did not improve this run. Compared with baseline, layer TTFT was 95.2% higher, total latency 4.5% higher, and generation throughput 12.1% lower; keep-alive should not be claimed as a speedup.
- Cold-load observations (`load_duration` > 50ms): prompt 1 layer 0.223s, prompt 2 layer 0.142s, prompt 3 layer 0.274s, prompt 4 layer 0.115s, prompt 5 layer 0.154s, prompt 6 layer 0.171s, prompt 7 layer 0.144s, prompt 8 layer 0.063s, prompt 9 layer 0.218s, prompt 10 layer 0.114s, prompt 11 layer 0.052s, prompt 12 layer 0.121s, prompt 14 layer 0.122s, prompt 19 layer 0.122s, prompt 20 layer 0.064s

## Environment and skipped checks

- GPU/CPU split is reported from `ollama ps`; it reflects the loaded benchmark model, not models that were not loaded.
- This harness does not load or unload the heavy profile. Heavy-model JSON/tool-call and GPU-offload checks remain pending explicit approval to swap the heavy model.
- FAST profile models absent from `ollama list` are not benchmarked in Phase 1; the benchmark uses the configured `fast.chat` model for all task categories as a safe baseline.
- Optimization verdict: compare aggregate table. If latency or throughput is not better, keep-alive did not demonstrate a speedup in this run.
