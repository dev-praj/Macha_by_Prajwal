import { Bridge, type DictationStats, type DictationTotals } from "../core/bridge";
import { h, clear } from "../views/dom";

type Period = "week" | "month" | "all";

const number = new Intl.NumberFormat();

function duration(minutes: number): string {
  const rounded = Math.round(minutes);
  if (rounded >= 60) {
    return `${Math.floor(rounded / 60)} h ${String(rounded % 60).padStart(2, "0")} min`;
  }
  return `${rounded} min`;
}

function applicationName(executable: string): string {
  const name = executable.replace(/\.exe$/i, "");
  const known: Record<string, string> = {
    code: "Visual Studio Code",
    cursor: "Cursor",
    devenv: "Visual Studio",
    firefox: "Firefox",
    chrome: "Google Chrome",
    msedge: "Microsoft Edge",
    outlook: "Outlook",
    slack: "Slack",
    teams: "Microsoft Teams",
    windowsterminal: "Windows Terminal",
  };
  return known[name.toLowerCase()] ?? name;
}

function metric(label: string, value: string, detail?: string): HTMLElement {
  const card = h(
    "div",
    { class: "dictation-stat-card" },
    h("span", { class: "dictation-stat-label", text: label }),
    h("strong", { text: value }),
  );
  if (detail) card.append(h("span", { class: "dictation-stat-detail", text: detail }));
  return card;
}

function totalsFor(stats: DictationStats, period: Period): DictationTotals {
  return stats[period];
}

function renderChart(stats: DictationStats, period: Period): HTMLElement {
  const days = stats.daily.slice(period === "week" ? -7 : -30);
  const maximum = Math.max(1, ...days.map(([, words]) => words));
  const bars = h("div", { class: "dictation-chart-bars", role: "img", "aria-label": "Words dictated per day" });
  for (const [date, words] of days) {
    const bar = h("span", {
      class: words === 0 ? "dictation-chart-bar zero" : "dictation-chart-bar",
      title: `${date} · ${number.format(words)} words`,
      "aria-label": `${date}: ${number.format(words)} words`,
    });
    bar.style.height = `${words === 0 ? 2 : Math.max(4, (words / maximum) * 100)}%`;
    bars.append(bar);
  }
  const axis = h("div", { class: "dictation-chart-axis", "aria-hidden": "true" });
  days.forEach(([date], index) => {
    if (period === "week" || index % 5 === 0 || index === days.length - 1) {
      axis.append(h("span", { text: String(Number(date.slice(-2))) }));
    }
  });
  return h(
    "div",
    { class: "dictation-stat-panel" },
    h("h3", { text: "Words per day" }),
    bars,
    axis,
  );
}

function renderApplications(stats: DictationStats): HTMLElement {
  const allApps = Object.entries(stats.apps).sort((left, right) => right[1] - left[1]);
  const total = allApps.reduce((sum, [, words]) => sum + words, 0);
  const entries = allApps.slice(0, 8);
  const panel = h(
    "div",
    { class: "dictation-stat-panel" },
    h("h3", { text: "Where you dictate" }),
  );
  if (entries.length === 0) {
    panel.append(h("p", { class: "dictation-stat-empty", text: "Dictate in an app and it will appear here." }));
    return panel;
  }
  for (const [executable, words] of entries) {
    const share = total > 0 ? (words / total) * 100 : 0;
    const row = h(
      "div",
      { class: "dictation-app-row" },
      h(
        "div",
        { class: "dictation-app-label" },
        h("span", { text: applicationName(executable) }),
        h("span", { text: `${Math.round(share)}%` }),
      ),
      h("div", { class: "dictation-app-track" }, h("i", { style: `width:${Math.max(2, share)}%` })),
    );
    panel.append(row);
  }
  return panel;
}

function renderStats(target: HTMLElement, stats: DictationStats, period: Period): void {
  clear(target);
  const totals = totalsFor(stats, period);
  target.append(
    h(
      "div",
      { class: "dictation-stat-grid" },
      metric("Words dictated", number.format(totals.words)),
      metric("Time saved", duration(totals.minutesSaved), "Compared with typing at 45 words/min"),
      metric("Speaking speed", number.format(totals.wordsPerMinute), "words/min"),
      metric("Dictations", number.format(totals.dictations)),
    ),
    h(
      "div",
      { class: "dictation-stat-panels" },
      renderChart(stats, period),
      renderApplications(stats),
    ),
    h("p", { class: "dictation-stat-privacy", text: "Counts stay on this computer. Dictation text and audio are not stored." }),
  );
}

export function dictationStatisticsSection(): HTMLElement {
  const status = h("p", { class: "hint", text: "Loading dictation statistics…" });
  const content = h("div", {});
  const refresh = h("button", { class: "secondary", text: "Refresh statistics", type: "button" });
  const periods = h("div", { class: "dictation-periods", role: "group", "aria-label": "Statistics period" });
  const buttons = new Map<Period, HTMLButtonElement>();
  let selected: Period = "month";
  let stats: DictationStats | null = null;

  const draw = () => {
    for (const [period, button] of buttons) {
      const active = period === selected;
      button.classList.toggle("on", active);
      button.setAttribute("aria-pressed", String(active));
    }
    if (stats) renderStats(content, stats, selected);
  };

  for (const [period, label] of [
    ["week", "Week"],
    ["month", "Month"],
    ["all", "All time"],
  ] as const) {
    const button = h("button", { class: "secondary", text: label, type: "button" });
    button.addEventListener("click", () => {
      selected = period;
      draw();
    });
    buttons.set(period, button);
    periods.append(button);
  }

  const section = h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Dictation statistics" })),
    h("p", { class: "hint", text: "See your dictation totals, speaking speed, daily usage, and the apps you dictate in." }),
    h("div", { class: "dictation-stat-controls" }, periods, refresh),
    status,
    content,
  );
  const load = async () => {
    status.textContent = "Loading dictation statistics…";
    section.insertBefore(status, content);
    refresh.disabled = true;
    try {
      stats = await Bridge.nixiDictationStats();
      status.remove();
      draw();
    } catch (error) {
      status.textContent = `Could not load dictation statistics: ${String(error).replace(/^Error:\s*/, "")}`;
    } finally {
      refresh.disabled = false;
    }
  };
  refresh.addEventListener("click", () => void load());
  void load();
  return section;
}
