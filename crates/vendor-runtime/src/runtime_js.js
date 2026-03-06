// JS shim that creates the sandboxed Nisaba global API from Deno ops.
((globalThis) => {
  const core = Deno.core;

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
      const resultJson = await core.ops.op_nisaba_fetch(url, optionsJson);
      const data = JSON.parse(resultJson);
      return new NisabaResponse(data);
    },

    sleep(ms) {
      return core.ops.op_nisaba_sleep(ms);
    },

    emitBatch(listings) {
      core.ops.op_nisaba_emit_batch(JSON.stringify(listings));
    },

    log: {
      trace(msg) { core.ops.op_nisaba_log("trace", String(msg)); },
      debug(msg) { core.ops.op_nisaba_log("debug", String(msg)); },
      info(msg) { core.ops.op_nisaba_log("info", String(msg)); },
      warn(msg) { core.ops.op_nisaba_log("warn", String(msg)); },
      error(msg) { core.ops.op_nisaba_log("error", String(msg)); },
    },
  };
})(globalThis);
