#!/usr/bin/env bash
# Script to build and register agent models in Ollama on Linux / macOS / WSL
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODELFILES_DIR="$SCRIPT_DIR/modelfiles"

declare -A AGENTS=(
    ["agent-router"]="Modelfile.router"
    ["agent-coder"]="Modelfile.coder"
    ["agent-summarizer"]="Modelfile.summarizer"
    ["agent-general"]="Modelfile.general"
)

echo "Creating agent models from Modelfiles..."

for name in "agent-router" "agent-coder" "agent-summarizer" "agent-general"; do
    file="$MODELFILES_DIR/${AGENTS[$name]}"
    if [ -f "$file" ]; then
        echo "Creating $name from ${AGENTS[$name]}..."
        ollama create "$name" -f "$file"
        echo ""
    else
        echo "[ERROR] Modelfile not found: $file"
    fi
done

echo "All agents processed. Current Ollama models:"
ollama list
