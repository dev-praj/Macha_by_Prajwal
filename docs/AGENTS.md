# Coding-agent integrations

In Settings → Agents, install **GitHub Copilot (VS Code & CLI)** to add Macha's
user hook file at `%USERPROFILE%\\.copilot\\hooks\\macha.json`. Visual Studio
Code Copilot Chat in **Copilot** session target and Copilot CLI use the same
Copilot harness and user hook directory, so a separate VS Code hook install is
not needed. Start a new VS Code agent session after installation.

Macha previews the exact hook changes and creates a backup before the user
confirms installation. Uninstall removes only Macha's entries. Copilot sessions,
tool activity, and permission requests are relayed to the Macha island; approval
responses use Copilot's `permissionDecision` format. If Macha is unavailable or
no decision is made before timeout, no decision is emitted and Copilot's normal
permission flow remains in charge.

VS Code's Copilot hook support is in preview and follows the shared Copilot
harness. See the [VS Code hooks documentation](https://code.visualstudio.com/docs/agent-customization/hooks)
and [GitHub Copilot hooks reference](https://docs.github.com/en/copilot/reference/hooks-reference).
