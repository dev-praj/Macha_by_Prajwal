import { Bridge, type AgentPlan, type AgentPlanWindow, type AgentProviderUsage } from "../core/bridge";
import { h, clear } from "../views/dom";

function resetLabel(resetsAtMs: number, weekly: boolean): string {
  const reset = new Date(resetsAtMs);
  if (!Number.isFinite(reset.getTime())) return "Reset time unavailable";
  const remaining = reset.getTime() - Date.now();
  if (remaining <= 0) return "Resetting";
  if (weekly) {
    return `Resets ${new Intl.DateTimeFormat(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    }).format(reset)}`;
  }
  const minutes = Math.ceil(remaining / 60_000);
  const hours = Math.floor(minutes / 60);
  return hours > 0
    ? `Resets in ${hours} h ${minutes % 60} min`
    : `Resets in ${minutes} min`;
}

function usageWindow(label: string, window: AgentPlanWindow | null, weekly = false): HTMLElement | null {
  if (!window) return null;
  const pct = Math.max(0, Math.min(100, Math.round(window.pct)));
  const fill = h("i", { class: "agent-usage-fill", style: `width:${pct}%` });
  fill.classList.toggle("near-limit", pct >= 75);
  fill.classList.toggle("at-limit", pct >= 90);
  const track = h(
    "div",
    { class: "agent-usage-track", role: "progressbar", "aria-label": `${label} usage`, "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(pct) },
    fill,
  );
  return h(
    "div",
    { class: "agent-usage-window" },
    h("div", { class: "agent-usage-window-heading" }, h("span", { text: label }), h("strong", { text: `${pct}%` })),
    track,
    h("span", { class: "hint", text: resetLabel(window.resetsAtMs, weekly) }),
  );
}

function providerCard(name: string, provider: AgentProviderUsage): HTMLElement {
  const card = h("article", { class: "agent-usage-card" }, h("h3", { text: name }));
  const plan: AgentPlan | null = provider.usage;
  if (!plan) {
    card.append(h("p", { class: provider.error ? "agent-usage-error" : "hint", text: provider.error ?? "No usage data available." }));
    return card;
  }

  if (plan.plan) card.append(h("p", { class: "agent-usage-plan", text: plan.plan }));
  const windows = h("div", { class: "agent-usage-windows" });
  const fiveHour = usageWindow("5 hours", plan.fiveHour);
  const sevenDay = usageWindow("Week", plan.sevenDay, true);
  if (fiveHour) windows.append(fiveHour);
  if (sevenDay) windows.append(sevenDay);
  card.append(windows);
  if (plan.resetCredits !== null && plan.resetCredits > 0) {
    card.append(h("p", { class: "hint", text: `${plan.resetCredits} Codex reset credits` }));
  }
  card.append(h("p", {
    class: "agent-usage-updated",
    text: `Updated ${new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(plan.updatedAtMs)}`,
  }));
  if (provider.error) card.append(h("p", { class: "agent-usage-error", text: provider.error }));
  return card;
}

export function agentPlanUsageSection(): HTMLElement {
  const status = h("p", { class: "hint", text: "Loading plan usage…" });
  const cards = h("div", { class: "agent-usage-grid" });
  const refresh = h("button", { type: "button", class: "secondary", text: "Refresh usage" });
  let loading = false;
  const section = h(
    "section",
    { class: "agent-usage-section" },
    h("div", { class: "agent-usage-title" }, h("h2", {}, h("span", { text: "Agent plan usage" })), refresh),
    h("p", {
      class: "hint",
      text: "Usage refreshes every 15 minutes while Settings is open. It uses your existing Claude Code and Codex sign-ins; Macha does not save or log their access tokens.",
    }),
    status,
    cards,
  );

  const load = async () => {
    if (loading) return;
    loading = true;
    status.className = "hint";
    status.textContent = "Checking Claude and Codex usage…";
    status.hidden = false;
    refresh.disabled = true;
    clear(cards);
    try {
      const report = await Bridge.agentPlanUsage();
      status.hidden = true;
      cards.append(providerCard("Claude", report.claude), providerCard("Codex", report.codex));
    } catch (error) {
      status.className = "agent-usage-error";
      status.textContent = `Could not check plan usage: ${String(error).replace(/^Error:\s*/, "")}`;
      status.hidden = false;
    } finally {
      loading = false;
      refresh.disabled = false;
    }
  };
  refresh.addEventListener("click", () => void load());
  window.setInterval(() => void load(), 15 * 60 * 1000);
  void load();
  return section;
}
