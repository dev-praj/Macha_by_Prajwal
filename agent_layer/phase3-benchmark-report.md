# Ollama benchmark

- Model: `qwen2.5:7b`
- Installed models: `agent-general:latest`, `agent-summarizer:latest`, `agent-coder:latest`, `agent-router:latest`, `gpt-oss:20b`, `gemma4:26b`, `qwen2.5:7b`
- Loaded models before benchmark: `qwen2.5:7b`
- `ollama ps` PROCESSOR: `100% GPU`
- Prompts: 20 fixed; both paths stream and use the same model and prompt.
- Baseline omits `keep_alive`; layer uses the configured profile `keep_alive` value. The request loops apply no additional warm-up, model swapping, prompt rewriting, or output cap.
- Qwen was already loaded and had been warmed at the configured 4096-token context before the run. This measures steady-state behavior; the two paths share Ollama's existing model residency.
- To reduce exact-prompt KV-cache order bias, the baseline runs in fixed order and the layer runs in reverse order.
- All 20 prompts use the configured benchmark model to keep comparisons apples-to-apples; this harness does not test agent-specific model routing.
- Configured profile models missing from `ollama list`: `llama3.2:3b`.
- Wall-clock benchmark duration: 193.3s.

| # | Task | Baseline TTFT (s) | Layer TTFT (s) | Baseline tok/s | Layer tok/s | Baseline total (s) | Layer total (s) | Cold load (base/layer, s) | JSON valid (base/layer) |
|---:|---|---:|---:|---:|---:|---:|---:|---:|:---:|
| 1 | chat | 0.438 | 0.152 | 16.760 | 14.836 | 3.390 | 4.050 | 0.009/0.020 | n/a |
| 2 | chat | 0.398 | 0.142 | 16.918 | 14.710 | 3.415 | 3.543 | 0.011/0.016 | n/a |
| 3 | chat | 0.579 | 0.758 | 16.829 | 15.387 | 6.748 | 5.483 | 0.169/0.015 | n/a |
| 4 | chat | 0.402 | 0.142 | 18.377 | 16.663 | 1.436 | 1.165 | 0.010/0.025 | n/a |
| 5 | voice | 0.242 | 0.136 | 19.291 | 16.027 | 0.948 | 1.142 | 0.009/0.019 | n/a |
| 6 | voice | 0.434 | 0.124 | 17.150 | 15.060 | 1.369 | 1.387 | 0.007/0.024 | n/a |
| 7 | voice | 0.392 | 0.127 | 17.873 | 15.336 | 1.960 | 1.238 | 0.014/0.027 | n/a |
| 8 | voice | 0.420 | 0.163 | 19.796 | 17.506 | 0.881 | 0.678 | 0.023/0.041 | n/a |
| 9 | automation | 0.275 | 0.371 | 18.096 | 15.499 | 1.658 | 1.989 | 0.045/0.096 | yes/yes |
| 10 | automation | 0.426 | 0.217 | 15.443 | 15.019 | 2.238 | 2.030 | 0.008/0.018 | yes/yes |
| 11 | automation | 0.497 | 0.525 | 15.618 | 16.779 | 2.554 | 2.286 | 0.018/0.196 | yes/yes |
| 12 | automation | 0.688 | 0.131 | 16.948 | 15.050 | 2.817 | 2.527 | 0.100/0.014 | yes/yes |
| 13 | code | 0.408 | 0.433 | 15.279 | 17.164 | 1.801 | 1.772 | 0.013/0.012 | n/a |
| 14 | code | 0.505 | 0.113 | 17.615 | 16.671 | 1.646 | 1.312 | 0.074/0.015 | n/a |
| 15 | code | 0.425 | 0.253 | 17.480 | 17.455 | 1.634 | 1.453 | 0.007/0.153 | n/a |
| 16 | code | 0.493 | 0.108 | 14.651 | 16.874 | 1.792 | 1.299 | 0.022/0.015 | n/a |
| 17 | summarize | 0.365 | 0.109 | 16.474 | 16.967 | 1.645 | 1.350 | 0.029/0.013 | n/a |
| 18 | summarize | 0.431 | 0.242 | 17.075 | 16.052 | 1.953 | 1.910 | 0.014/0.008 | n/a |
| 19 | complex | 0.285 | 0.697 | 14.903 | 15.423 | 28.679 | 32.664 | 0.013/0.221 | n/a |
| 20 | complex | 0.679 | 0.256 | 14.801 | 15.257 | 29.396 | 25.939 | 0.058/0.030 | n/a |

## Aggregate

| Metric | Baseline mean | Layer mean | Relative layer change |
|---|---:|---:|---:|
| TTFT (s) | 0.439 | 0.260 | -40.8% |
| Total latency (s) | 4.898 | 4.761 | -2.8% |
| Generation (tokens/s) | 16.869 | 15.987 | -5.2% |
| Mean cold load when observed (s) | 0.100 | 0.166 | +66.0% |
| Steady-state latency (s) | 4.865 | 4.712 | -3.2% |

- Automation JSON validity: baseline 4/4, layer 4/4.
- Cold-load observations (`load_duration` > 50ms): prompt 3 baseline 0.169s, prompt 9 layer 0.096s, prompt 11 layer 0.196s, prompt 12 baseline 0.100s, prompt 14 baseline 0.074s, prompt 15 layer 0.153s, prompt 19 layer 0.221s, prompt 20 baseline 0.058s

## Environment and skipped checks

- GPU/CPU split is reported from `ollama ps`; it reflects the loaded benchmark model, not models that were not loaded.
- This harness does not load or unload the heavy profile. Heavy-model JSON/tool-call and GPU-offload checks remain pending explicit approval to swap the heavy model.
- FAST profile models absent from `ollama list` are not benchmarked; the benchmark uses the configured `fast.chat` model for all task categories as a safe baseline.
- Optimization verdict: TTFT was 40.8% lower and total latency 2.8% lower in this single run, while generation throughput was 5.2% lower. Mean observed cold-load duration was 66.0% higher for the layer. Since the model was already resident and prompt order/cache effects remain, these mixed measurements do not establish a general keep-alive speedup.
