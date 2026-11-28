(() => {
  const hostId = "__eefi_reader_mode";
  const currentHost = document.getElementById(hostId);
  if (currentHost) {
    currentHost.dispatchEvent(new Event("eefi-reader-mode-close"));
    currentHost.remove();
    return;
  }
  if (!/^https?:$/.test(location.protocol) || !document.body) return;

  const excludedTags = new Set([
    "SCRIPT", "STYLE", "NOSCRIPT", "NAV", "HEADER", "FOOTER", "ASIDE", "FORM",
    "BUTTON", "INPUT", "SELECT", "TEXTAREA", "IFRAME", "SVG", "CANVAS", "VIDEO",
    "AUDIO", "TEMPLATE",
  ]);
  const allowedTags = new Set([
    "ARTICLE", "SECTION", "DIV", "P", "H1", "H2", "H3", "H4", "H5", "H6",
    "UL", "OL", "LI", "BLOCKQUOTE", "PRE", "CODE", "A", "IMG", "FIGURE",
    "FIGCAPTION", "HR", "BR", "TABLE", "THEAD", "TBODY", "TR", "TH", "TD",
    "STRONG", "B", "EM", "I", "S", "DEL", "INS", "SUP", "SUB",
  ]);

  function visible(element) {
    const style = getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" &&
      element.getClientRects().length > 0;
  }

  function score(element) {
    if (!visible(element)) return -1;
    const text = (element.innerText || "").trim();
    if (text.length < 180) return -1;
    const paragraphs = element.querySelectorAll("p").length;
    const linkText = Array.from(element.querySelectorAll("a"))
      .reduce((length, link) => length + (link.innerText || "").trim().length, 0);
    const semanticBoost = element.tagName === "ARTICLE" || element.tagName === "MAIN" ||
      element.getAttribute("role") === "main" ? 1400 : 0;
    return text.length + paragraphs * 120 + semanticBoost - linkText * 0.65;
  }

  const candidates = Array.from(
    document.querySelectorAll("article, main, [role='main'], section, div"),
  ).slice(0, 300);
  let articleSource = document.body;
  let bestScore = score(articleSource);
  for (const candidate of candidates) {
    const candidateScore = score(candidate);
    if (candidateScore > bestScore) {
      articleSource = candidate;
      bestScore = candidateScore;
    }
  }

  function safeWebUrl(value) {
    try {
      const url = new URL(value, document.baseURI);
      return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
    } catch {
      return null;
    }
  }

  function copyContent(source, destination, budget, depth = 0) {
    if (budget.nodes >= 6000 || budget.characters >= 1000000 || depth > 64) return;
    if (source.nodeType === Node.TEXT_NODE) {
      const remaining = 1000000 - budget.characters;
      const text = source.nodeValue.slice(0, remaining);
      if (text) {
        destination.appendChild(document.createTextNode(text));
        budget.characters += text.length;
      }
      return;
    }
    if (source.nodeType !== Node.ELEMENT_NODE) return;

    const tag = source.tagName;
    if (excludedTags.has(tag)) return;
    budget.nodes += 1;
    if (!allowedTags.has(tag)) {
      for (const child of source.childNodes) {
        copyContent(child, destination, budget, depth + 1);
        if (budget.nodes >= 6000 || budget.characters >= 1000000) break;
      }
      return;
    }

    if (tag === "IMG") {
      if (!source.complete || !source.naturalWidth) return;
      const src = safeWebUrl(source.currentSrc || source.src);
      if (!src) return;
    }

    const clean = document.createElement(tag.toLowerCase());
    if (tag === "A") {
      const href = safeWebUrl(source.getAttribute("href") || "");
      if (href) {
        clean.href = href;
        clean.rel = "noopener noreferrer";
      }
    } else if (tag === "IMG") {
      clean.src = safeWebUrl(source.currentSrc || source.src);
      clean.alt = source.alt || "";
      clean.loading = "lazy";
      clean.referrerPolicy = "no-referrer";
    }
    destination.appendChild(clean);
    for (const child of source.childNodes) {
      copyContent(child, clean, budget, depth + 1);
      if (budget.nodes >= 6000 || budget.characters >= 1000000) break;
    }
  }

  const host = document.createElement("div");
  host.id = hostId;
  host.style.cssText = "position:fixed;inset:0;z-index:2147483647;";
  const shadow = host.attachShadow({ mode: "closed" });
  shadow.innerHTML = `
    <style>
      :host { all: initial; }
      .reader {
        --background: #fff;
        --foreground: #202124;
        --muted: #656b73;
        --surface: #f2f3f5;
        position: fixed;
        inset: 0;
        overflow: auto;
        color: var(--foreground);
        background: var(--background);
        color-scheme: light;
        font: 16px/1.5 system-ui, sans-serif;
      }
      .reader[data-theme="sepia"] {
        --background: #f4ecd8;
        --foreground: #352b1e;
        --muted: #776a57;
        --surface: #e9dfc9;
        color-scheme: light;
      }
      .reader[data-theme="dark"] {
        --background: #191b1e;
        --foreground: #e6e3dc;
        --muted: #a7a39a;
        --surface: #282b30;
        color-scheme: dark;
      }
      .toolbar {
        position: sticky;
        top: 0;
        z-index: 1;
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 52px;
        padding: 8px max(16px, calc((100vw - 1100px) / 2));
        border-bottom: 1px solid color-mix(in srgb, var(--muted) 24%, transparent);
        background: var(--background);
      }
      .brand { font-weight: 700; white-space: nowrap; }
      .title {
        overflow: hidden;
        flex: 1;
        color: var(--muted);
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      button {
        min-height: 34px;
        padding: 5px 10px;
        border: 0;
        border-radius: 6px;
        color: var(--foreground);
        background: var(--surface);
        font: inherit;
        cursor: pointer;
      }
      button:hover { filter: brightness(.94); }
      button:focus-visible { outline: 2px solid #536fce; outline-offset: 2px; }
      .article {
        box-sizing: border-box;
        max-width: 760px;
        margin: 48px auto 96px;
        padding: 0 24px;
        font: 18px/1.8 Georgia, "Times New Roman", serif;
        overflow-wrap: anywhere;
      }
      .article h1, .article h2, .article h3, .article h4 {
        line-height: 1.25;
        margin: 1.5em 0 .6em;
      }
      .article h1 { margin-top: 0; font-size: 2em; }
      .article p { margin: 0 0 1.2em; }
      .article a { color: #4263b5; }
      .reader[data-theme="dark"] .article a { color: #a9bcff; }
      .article img { display: block; max-width: 100%; height: auto; margin: 24px auto; }
      .article figure { margin: 28px 0; }
      .article figcaption { color: var(--muted); font: 14px/1.5 system-ui, sans-serif; }
      .article pre { overflow: auto; padding: 16px; border-radius: 8px; background: var(--surface); }
      .article blockquote { margin: 1.5em 0; padding-left: 1em; border-left: 3px solid var(--muted); }
      .article table { border-collapse: collapse; max-width: 100%; }
      .article th, .article td { padding: 6px 10px; border: 1px solid var(--muted); }
      .empty { color: var(--muted); font: 16px/1.6 system-ui, sans-serif; }
      @media (max-width: 720px) {
        .toolbar { flex-wrap: wrap; padding: 8px 12px; }
        .title { flex-basis: 100%; order: 2; }
        .article { margin: 28px auto 64px; padding: 0 18px; }
      }
    </style>
    <div class="reader" data-theme="light">
      <header class="toolbar" aria-label="Reader mode controls">
        <span class="brand">Reader mode</span>
        <span class="title"></span>
        <button type="button" data-action="smaller" aria-label="Decrease text size">A-</button>
        <button type="button" data-action="larger" aria-label="Increase text size">A+</button>
        <button type="button" data-action="narrower" aria-label="Narrower text column">Width -</button>
        <button type="button" data-action="wider" aria-label="Wider text column">Width +</button>
        <button type="button" data-action="theme" aria-label="Change reader theme">Theme: Light</button>
        <button type="button" data-action="close" aria-label="Exit reader mode">Exit</button>
      </header>
      <main class="article"></main>
    </div>`;

  const reader = shadow.querySelector(".reader");
  const article = shadow.querySelector(".article");
  const title = shadow.querySelector(".title");
  title.textContent = document.title.trim() || location.hostname;
  const content = document.createElement("div");
  copyContent(articleSource, content, { nodes: 0, characters: 0 });
  if (content.textContent.trim().length < 100) {
    const message = document.createElement("p");
    message.className = "empty";
    message.textContent = "No readable article text was found on this page.";
    article.appendChild(message);
  } else {
    article.appendChild(content);
  }

  let fontSize = 18;
  let columnWidth = 760;
  const themes = ["light", "sepia", "dark"];
  let themeIndex = 0;
  const themeButton = shadow.querySelector('[data-action="theme"]');
  function updateLayout() {
    article.style.fontSize = `${fontSize}px`;
    article.style.maxWidth = `${columnWidth}px`;
    reader.dataset.theme = themes[themeIndex];
    themeButton.textContent = `Theme: ${themes[themeIndex][0].toUpperCase()}${themes[themeIndex].slice(1)}`;
  }

  shadow.querySelector('[data-action="smaller"]').addEventListener("click", () => {
    fontSize = Math.max(14, fontSize - 1);
    updateLayout();
  });
  shadow.querySelector('[data-action="larger"]').addEventListener("click", () => {
    fontSize = Math.min(28, fontSize + 1);
    updateLayout();
  });
  shadow.querySelector('[data-action="narrower"]').addEventListener("click", () => {
    columnWidth = Math.max(560, columnWidth - 80);
    updateLayout();
  });
  shadow.querySelector('[data-action="wider"]').addEventListener("click", () => {
    columnWidth = Math.min(1120, columnWidth + 80);
    updateLayout();
  });
  themeButton.addEventListener("click", () => {
    themeIndex = (themeIndex + 1) % themes.length;
    updateLayout();
  });
  const close = () => {
    host.dispatchEvent(new Event("eefi-reader-mode-close"));
    host.remove();
  };
  shadow.querySelector('[data-action="close"]').addEventListener("click", close);
  const onKeyDown = (event) => {
    if (event.key === "Escape") close();
  };
  host.addEventListener(
    "eefi-reader-mode-close",
    () => window.removeEventListener("keydown", onKeyDown, true),
    { once: true },
  );
  window.addEventListener("keydown", onKeyDown, true);
  document.documentElement.appendChild(host);
  updateLayout();
})();
