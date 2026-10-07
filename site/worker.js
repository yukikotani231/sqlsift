// Runs the sqlsift WebAssembly module off the main thread. If the module
// traps (e.g. an internal panic), the page terminates and recreates this worker.
import init, { check, version } from './pkg/sqlsift_wasm.js';

const ready = init().then(
  () => postMessage({ type: 'ready', version: version() }),
  (err) => {
    postMessage({ type: 'fatal', message: String(err && err.message ? err.message : err) });
    throw err;
  },
);

self.onmessage = async (event) => {
  const { id, schema, query, dialect } = event.data;
  try {
    await ready;
  } catch {
    return;
  }
  try {
    const t0 = performance.now();
    const json = check(schema, query, dialect);
    const ms = performance.now() - t0;
    postMessage({ type: 'result', id, result: JSON.parse(json), ms });
  } catch (err) {
    postMessage({ type: 'crash', id, message: String(err && err.message ? err.message : err) });
  }
};
