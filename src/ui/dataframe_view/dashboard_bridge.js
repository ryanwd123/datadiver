// DataDiver's side of a dashboard page, run before the page's own scripts.
// It defines `window.datadiver`; docs/dashboards/agent-guide.md describes it.
(() => {
  "use strict";
  if (window.datadiver || !window.chrome || !window.chrome.webview) return;
  const host = window.chrome.webview;
  const pending = new Map();
  const handlers = new Map();
  let nextId = 1;
  let config = null;
  let markReady;
  const ready = new Promise((resolve) => { markReady = resolve; });

  const post = (message) => host.postMessage(message);

  const text = (value) => {
    if (value instanceof Error) return value.stack || String(value);
    if (typeof value === "string") return value;
    try { return JSON.stringify(value); } catch { return String(value); }
  };
  const report = (level, values) => {
    try { post({ type: "log", level, message: values.map(text).join(" ") }); } catch { /* the host is gone */ }
  };
  for (const level of ["error", "warn"]) {
    const original = console[level].bind(console);
    console[level] = (...values) => { report(level, values); original(...values); };
  }
  window.addEventListener("error", (event) => report("error", [event.error || event.message]));
  window.addEventListener("unhandledrejection", (event) => report("error", ["Unhandled promise rejection:", event.reason]));

  const request = (type, payload) => {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject, payload });
      post(Object.assign({ type, id }, payload));
    });
  };

  const runCommand = (id) => {
    const list = handlers.get(id) || [];
    if (!list.length) {
      console.warn(`DataDiver: nothing handles the command "${id}"; add datadiver.onCommand("${id}", handler)`);
    }
    for (const handler of list) {
      try { handler(); } catch (error) { report("error", [error]); }
    }
  };

  host.addEventListener("message", (event) => {
    const message = event.data;
    if (!message || typeof message !== "object") return;
    if (message.type === "config") {
      config = message;
      markReady(config);
    } else if (message.type === "command") {
      runCommand(message.id);
    } else if (message.type === "reply") {
      const entry = pending.get(message.id);
      if (!entry) return;
      pending.delete(message.id);
      if (message.error !== undefined) {
        const error = new Error(message.error);
        error.sql = entry.payload.sql;
        entry.reject(error);
      } else {
        entry.resolve(message.result);
      }
    }
  });

  // Keys in DataDiver's form, such as `ctrl-shift-r`.
  const named = {
    Enter: "enter", Escape: "escape", Tab: "tab", Backspace: "backspace", Delete: "delete",
    Insert: "insert", Home: "home", End: "end", PageUp: "pageup", PageDown: "pagedown",
    ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down", Space: "space",
    Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";",
    Quote: "'", Comma: ",", Period: ".", Slash: "/", Backquote: "`", NumpadAdd: "+",
    NumpadSubtract: "-", NumpadMultiply: "*", NumpadDivide: "/", NumpadDecimal: ".", NumpadEnter: "enter",
  };
  const keyName = (event) => {
    const code = event.code || "";
    if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
    if (/^(Digit|Numpad)[0-9]$/.test(code)) return code.slice(-1);
    if (/^F[0-9]{1,2}$/.test(code)) return code.toLowerCase();
    return named[code] || null;
  };
  const stroke = (event) => {
    const key = keyName(event);
    if (!key) return null;
    const parts = [];
    if (event.ctrlKey) parts.push("ctrl");
    if (event.altKey) parts.push("alt");
    if (event.shiftKey) parts.push("shift");
    if (event.metaKey) parts.push("cmd");
    parts.push(key);
    return parts.join("-");
  };
  const typing = (target) => {
    if (!target || !(target instanceof Element)) return false;
    if (target.isContentEditable) return true;
    const tag = target.tagName;
    return tag === "TEXTAREA" || tag === "SELECT" || (tag === "INPUT" && !["button", "checkbox", "radio", "range", "submit", "reset", "color"].includes(target.type));
  };
  window.addEventListener("keydown", (event) => {
    if (!config || !config.commands || event.repeat) return;
    const pressed = stroke(event);
    const command = pressed && config.commands.find((command) => command.key === pressed);
    if (!command) return;
    // A plain key types in a text box; only keys with Ctrl or Alt, or F-keys, run there.
    const modified = event.ctrlKey || event.altKey || event.metaKey || /^f[0-9]+$/.test(keyName(event));
    if (!modified && typing(event.target)) return;
    event.preventDefault();
    event.stopPropagation();
    runCommand(command.id);
  }, true);

  const toObjects = (columns, rows) => {
    const names = columns.map((column) => column.name);
    return rows.map((row) => {
      const object = {};
      names.forEach((name, index) => { object[name] = row[index]; });
      return object;
    });
  };

  window.datadiver = Object.freeze({
    version: 1,
    /** Resolves once DataDiver has sent the dashboard's settings. */
    ready,
    /**
     * Runs read-only SQL against the dashboard's sources.
     * params: {name: value} for $name, or [value, ...] for ? and $1. Lists bind as DuckDB lists.
     * options: {limit} rows, 10,000 by default.
     * Resolves to {columns, rows (objects), values (arrays), truncated, elapsedMs}.
     */
    query(sql, params, options) {
      if (typeof sql !== "string") return Promise.reject(new TypeError("datadiver.query needs the SQL as text"));
      const limit = options && options.limit !== undefined ? options.limit : null;
      return request("query", { sql, params: params === undefined ? null : params, limit })
        .then((result) => Object.assign(result, { values: result.rows, rows: toObjects(result.columns, result.rows) }));
    },
    /** The manifest's sources: [{alias, kind, location, error?}]. */
    sources() {
      return ready.then((settings) => settings.sources);
    },
    /** Runs handler when the command runs, from its key or the command palette. Returns a function that removes it. */
    onCommand(id, handler) {
      if (typeof handler !== "function") throw new TypeError("datadiver.onCommand needs a function");
      const list = handlers.get(id) || [];
      list.push(handler);
      handlers.set(id, list);
      return () => {
        const at = list.indexOf(handler);
        if (at >= 0) list.splice(at, 1);
      };
    },
  });

  post({ type: "hello", version: 1 });
})();
