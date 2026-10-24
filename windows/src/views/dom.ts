// Minimal DOM helpers — no framework, as specified.
import { onLanguageChange, t } from "../i18n/i18n";

onLanguageChange(() => {
  for (const el of document.querySelectorAll<HTMLElement>("[data-i18n-text], [data-i18n-title], [data-i18n-placeholder], [data-i18n-aria-label]")) {
    for (const attr of ["text", "title", "placeholder", "aria-label"] as const) {
      const dataKey = attr.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());
      const key = el.dataset[`i18n${dataKey[0].toUpperCase()}${dataKey.slice(1)}`];
      if (!key) continue;
      const translated = t(key);
      if (attr === "text") el.textContent = translated;
      else el.setAttribute(attr, translated);
    }
  }
});

type Attrs = Record<string, string | number | boolean | EventListener | undefined>;
type Child = Node | string | null | undefined | false;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = String(v);
    else if (k === "text") {
      el.dataset.i18nText = String(v);
      el.textContent = t(String(v));
    }
    else if (k === "html") el.innerHTML = String(v);
    else if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
    } else if (k === "style") el.setAttribute("style", String(v));
    else if (k === "title" || k === "placeholder" || k === "aria-label") {
      const dataKey = k.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());
      el.dataset[`i18n${dataKey[0].toUpperCase()}${dataKey.slice(1)}`] = String(v);
      el.setAttribute(k, t(String(v)));
    }
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children) {
    if (c == null || c === false) continue;
    el.append(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return el;
}

export function svg(path: string, size = 14, opts: { fill?: string; stroke?: number } = {}): SVGSVGElement {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("width", String(size));
  el.setAttribute("height", String(size));
  el.setAttribute("aria-hidden", "true");
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p.setAttribute("d", path);
  if (opts.stroke) {
    p.setAttribute("fill", "none");
    p.setAttribute("stroke", "currentColor");
    p.setAttribute("stroke-width", String(opts.stroke));
    p.setAttribute("stroke-linecap", "round");
    p.setAttribute("stroke-linejoin", "round");
  } else {
    p.setAttribute("fill", opts.fill ?? "currentColor");
  }
  el.append(p);
  return el;
}

export function clear(el: Element) {
  while (el.firstChild) el.removeChild(el.firstChild);
}

/** Card dot used in every "who" row. */
export function dot(color: string, size = 7): HTMLElement {
  return h("i", {
    class: "dot",
    style: `width:${size}px;height:${size}px;background:${color}`,
  });
}
