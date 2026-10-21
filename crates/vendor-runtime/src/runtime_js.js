// JS shim that creates the sandboxed Nisaba global API from Deno ops.
//
// This runs before any plugin code. It captures the ops it needs in this closure and then
// removes `Deno` from the global scope, so the `Nisaba` object is the plugin's *only* API:
// `Deno.core.ops` would otherwise expose every deno_core built-in op, including
// `op_panic`, which aborts the whole host process.
((globalThis) => {
  const core = Deno.core;
  const ops = core.ops;

  class NisabaResponse {
    constructor(data) {
      this._status = data.status;
      this._headers = data.headers;
      this._body = data.body;
    }

    get ok() {
      return this._status >= 200 && this._status < 300;
    }

    get status() {
      return this._status;
    }

    get headers() {
      return this._headers;
    }

    text() {
      return this._body;
    }

    json() {
      return JSON.parse(this._body);
    }
  }

  globalThis.Nisaba = {
    async fetch(url, options = {}) {
      const optionsJson = JSON.stringify({
        method: options.method || "GET",
        headers: options.headers || {},
        body: options.body || null,
      });
      const resultJson = await ops.op_nisaba_fetch(url, optionsJson);
      const data = JSON.parse(resultJson);
      return new NisabaResponse(data);
    },

    sleep(ms) {
      return ops.op_nisaba_sleep(ms);
    },

    emitBatch(listings) {
      ops.op_nisaba_emit_batch(JSON.stringify(listings));
    },

    // Per-plugin key-value storage for cursors, ETags and last-seen
    // timestamps. Keys are scoped to this plugin (and this company): a plugin
    // can never read another plugin's keys. Values are strings - stringify
    // objects first. 1 MiB total per plugin; `set` rejects past the cap.
    // Values persist between runs in the company database.
    store: {
      async get(key) {
        if (typeof key !== "string" || key.length === 0) {
          throw new TypeError("Nisaba.store.get: key must be a non-empty string");
        }
        const { found, value } = JSON.parse(await ops.op_nisaba_store_get(key));
        return found ? value : null;
      },

      async set(key, value) {
        if (typeof key !== "string" || key.length === 0) {
          throw new TypeError("Nisaba.store.set: key must be a non-empty string");
        }
        if (typeof value !== "string") {
          throw new TypeError("Nisaba.store.set: value must be a string - JSON.stringify objects first");
        }
        await ops.op_nisaba_store_set(key, value);
      },

      async delete(key) {
        if (typeof key !== "string" || key.length === 0) {
          throw new TypeError("Nisaba.store.delete: key must be a non-empty string");
        }
        return ops.op_nisaba_store_delete(key);
      },
    },

    log: {
      trace(msg) { ops.op_nisaba_log("trace", String(msg)); },
      debug(msg) { ops.op_nisaba_log("debug", String(msg)); },
      info(msg) { ops.op_nisaba_log("info", String(msg)); },
      warn(msg) { ops.op_nisaba_log("warn", String(msg)); },
      error(msg) { ops.op_nisaba_log("error", String(msg)); },
    },
  };

  // Plugin authors reach for console.log; deno_core's console prints to the host
  // process's stdout, which nobody sees in the desktop app. Route it into the same log as
  // Nisaba.log instead.
  const formatArgs = (args) =>
    args
      .map((arg) => {
        if (typeof arg === "string") return arg;
        try {
          const json = JSON.stringify(arg);
          if (json !== undefined) return json;
        } catch {
          // cyclic, or a BigInt - fall through
        }
        try {
          return String(arg);
        } catch {
          return "[unprintable]";
        }
      })
      .join(" ");
  const logAt = (level) => (...args) => ops.op_nisaba_log(level, formatArgs(args));
  globalThis.console = {
    log: logAt("info"),
    info: logAt("info"),
    debug: logAt("debug"),
    trace: logAt("trace"),
    warn: logAt("warn"),
    error: logAt("error"),
  };

  // How the host's wrapper module hands the plugin's result back to Rust. Reachable by
  // plugin code too, which gains nothing: it can already choose what it returns.
  Object.defineProperty(globalThis, "__nisabaSetResult", {
    value: (json) => ops.op_nisaba_set_result(json),
    enumerable: false,
    writable: false,
    configurable: false,
  });

  delete globalThis.Deno;
  delete globalThis.__bootstrap;
})(globalThis);
