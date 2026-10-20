"""Print optional Ollama server settings with their memory trade-offs."""

from __future__ import annotations


RECOMMENDATIONS = (
    (
        "OLLAMA_FLASH_ATTENTION",
        "1",
        "Can speed attention on supported backends; may use additional workspace memory.",
    ),
    (
        "OLLAMA_KV_CACHE_TYPE",
        "q8_0",
        "Reduces KV-cache memory; quantized cache can slightly affect output quality.",
    ),
    (
        "OLLAMA_NUM_PARALLEL",
        "1",
        "Keeps concurrent generation and KV-cache memory pressure low.",
    ),
    (
        "OLLAMA_MAX_LOADED_MODELS",
        "2",
        "For the fast profile only. Do not keep the heavy model beside fast models.",
    ),
)


def main() -> int:
    print("Suggested Ollama server environment variables (not applied automatically):")
    for name, value, explanation in RECOMMENDATIONS:
        print(f"  {name}={value}")
        print(f"    {explanation}")
    print(
        "\nRaising parallelism or the loaded-model limit can increase shared-memory "
        "and KV-cache use, lower tokens/sec, and cause model thrashing. Measure each "
        "change on your workload before increasing it."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
