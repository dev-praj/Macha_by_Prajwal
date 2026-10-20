# Ollama benchmark

- Model: `qwen2.5:7b`
- Installed models: `agent-general:latest`, `agent-summarizer:latest`, `agent-coder:latest`, `agent-router:latest`, `gpt-oss:20b`, `gemma4:26b`, `qwen2.5:7b`
- Loaded models before benchmark: `qwen2.5:7b`
- `ollama ps` PROCESSOR: `100% GPU`
- Prompts: 20 fixed; both paths stream and use the same model and prompt.
- Baseline omits `keep_alive`; layer uses the configured profile `keep_alive` value. The request loops apply no additional warm-up, model swapping, prompt rewriting, or output cap.
- To reduce exact-prompt KV-cache order bias, the baseline runs in fixed order and the layer runs in reverse order.
- All 20 prompts use the configured benchmark model to keep comparisons apples-to-apples; this harness does not test agent-specific model routing.
- Configured profile models missing from `ollama list`: `llama3.2:3b`.
- Wall-clock benchmark duration: 162.0s.

| # | Task | Baseline TTFT (s) | Layer TTFT (s) | Baseline tok/s | Layer tok/s | Baseline total (s) | Layer total (s) | Cold load (base/layer, s) | JSON valid (base/layer) |
|---:|---|---:|---:|---:|---:|---:|---:|---:|:---:|
| 1 | chat | 0.368 | 0.097 | 20.469 | 17.224 | 2.664 | 2.536 | 0.008/0.011 | n/a |
| 2 | chat | 0.088 | 0.109 | 20.348 | 16.847 | 2.692 | 3.433 | 0.007/0.009 | n/a |
| 3 | chat | 0.088 | 0.384 | 20.263 | 16.720 | 4.036 | 7.741 | 0.006/0.007 | n/a |
| 4 | chat | 0.094 | 0.107 | 20.256 | 17.006 | 0.885 | 1.049 | 0.010/0.017 | n/a |
| 5 | voice | 0.110 | 0.116 | 20.866 | 17.329 | 0.730 | 0.866 | 0.015/0.016 | n/a |
| 6 | voice | 0.078 | 0.210 | 21.665 | 16.900 | 0.816 | 1.277 | 0.008/0.081 | n/a |
| 7 | voice | 0.082 | 0.099 | 20.846 | 17.617 | 1.234 | 1.763 | 0.009/0.015 | n/a |
| 8 | voice | 0.082 | 0.114 | 22.123 | 19.248 | 0.534 | 0.582 | 0.009/0.009 | n/a |
| 9 | automation | 0.097 | 0.112 | 20.756 | 17.695 | 1.300 | 1.524 | 0.009/0.010 | yes/yes |
| 10 | automation | 0.098 | 0.111 | 20.417 | 18.059 | 1.469 | 1.661 | 0.010/0.008 | yes/yes |
| 11 | automation | 0.097 | 0.128 | 20.198 | 17.800 | 1.682 | 1.926 | 0.009/0.006 | yes/yes |
| 12 | automation | 0.087 | 0.091 | 19.915 | 16.370 | 1.895 | 2.290 | 0.011/0.007 | yes/yes |
| 13 | code | 0.107 | 0.369 | 18.927 | 17.018 | 1.322 | 1.721 | 0.006/0.012 | n/a |
| 14 | code | 0.083 | 0.090 | 18.052 | 18.205 | 1.192 | 1.188 | 0.010/0.007 | n/a |
| 15 | code | 0.099 | 0.102 | 18.201 | 18.032 | 1.087 | 1.158 | 0.007/0.008 | n/a |
| 16 | code | 0.350 | 0.103 | 16.969 | 18.258 | 1.534 | 1.143 | 0.010/0.006 | n/a |
| 17 | summarize | 0.156 | 0.104 | 14.599 | 18.770 | 1.572 | 1.223 | 0.036/0.007 | n/a |
| 18 | summarize | 0.096 | 0.175 | 12.471 | 18.433 | 2.233 | 1.585 | 0.013/0.009 | n/a |
| 19 | complex | 0.546 | 0.433 | 13.802 | 17.217 | 25.478 | 22.099 | 0.168/0.009 | n/a |
| 20 | complex | 0.501 | 0.250 | 15.513 | 17.928 | 32.477 | 18.322 | 0.014/0.018 | n/a |

## Aggregate

| Metric | Baseline mean | Layer mean | Relative layer change |
|---|---:|---:|---:|
| TTFT (s) | 0.165 | 0.165 | -0.1% |
| Total latency (s) | 4.342 | 3.754 | -13.5% |
| Generation (tokens/s) | 18.833 | 17.634 | -6.4% |
| Mean cold load when observed (s) | 0.168 | 0.081 | -51.7% |
| Steady-state latency (s) | 4.323 | 3.741 | -13.5% |

- Automation JSON validity: baseline 4/4, layer 4/4.
- Cold-load observations (`load_duration` > 50ms): prompt 6 layer 0.081s, prompt 19 baseline 0.168s

## Environment and skipped checks

- GPU/CPU split is reported from `ollama ps`; it reflects the loaded benchmark model, not models that were not loaded.
- This harness does not load or unload the heavy profile. Heavy-model JSON/tool-call and GPU-offload checks remain pending explicit approval to swap the heavy model.
- FAST profile models absent from `ollama list` are not benchmarked; the benchmark uses the configured `fast.chat` model for all task categories as a safe baseline.
- Optimization verdict: report each aggregate metric separately. A single run with mixed results is not evidence of a general speedup.
